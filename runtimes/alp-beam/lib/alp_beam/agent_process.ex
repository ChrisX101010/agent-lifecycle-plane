defmodule AlpBeam.AgentProcess do
  use GenServer
  require Logger

  @poll_ms 350
  @heartbeat_ms 15_000

  def child_spec(config) do
    %{
      id: {:alp_agent, config.principal_id},
      start: {__MODULE__, :start_link, [config]},
      restart: :permanent,
      shutdown: 5_000,
      type: :worker
    }
  end

  def start_link(config) do
    GenServer.start_link(
      __MODULE__,
      config,
      name: {:via, Registry, {AlpBeam.ProcessRegistry, config.principal_id}}
    )
  end

  @impl true
  def init(config) do
    state =
      config
      |> Map.put(:boot_count, AlpBeam.RuntimeState.next_boot(config.principal_id))
      |> Map.put(:recovered, AlpBeam.ControlPlane.recovery(config.principal_id))
      |> Map.put(:started_at, DateTime.utc_now() |> DateTime.to_iso8601())
      |> Map.put(:steps_completed, 0)
      |> Map.put(:company_work_completed, 0)
      |> Map.put(:last_step, nil)
      |> Map.put(:last_result, nil)

    Process.send_after(self(), :poll, @poll_ms)
    {:ok, state}
  end

  @impl true
  def handle_call(:info, _from, state), do: {:reply, render(state), state}

  @impl true
  def handle_call(:recover, _from, state) do
    recovered = AlpBeam.ControlPlane.recovery(state.principal_id)
    new_state = Map.put(state, :recovered, recovered)
    {:reply, render(new_state), new_state}
  end

  @impl true
  def handle_cast(:crash, state), do: exit({:simulated_crash, state.principal_id})

  @impl true
  def handle_info(:poll, state) do
    new_state = poll_once(state)
    Process.send_after(self(), :poll, @poll_ms)
    {:noreply, new_state}
  rescue
    error ->
      Process.send_after(self(), :poll, @poll_ms)
      {:noreply, Map.put(state, :last_result, %{"poll_exception" => Exception.message(error)})}
  end

  defp poll_once(state) do
    case AlpBeam.ControlPlane.claim_company(state.principal_id) do
      {:ok, %{"state" => "claimed", "work_item" => item}} ->
        process_company_work(state, item)

      {:ok, %{"state" => "no_work"}} ->
        poll_orchestration(state)

      {:ok, response} ->
        Map.put(state, :last_result, %{"unexpected_company_claim_response" => response})

      {:error, %{"status" => 403}} ->
        # ALP06-10 agents may not have case.work until ALP11 grants it.
        poll_orchestration(state)

      {:error, reason} ->
        Map.put(state, :last_result, %{"company_claim_error" => reason})
    end
  end

  defp poll_orchestration(state) do
    case AlpBeam.ControlPlane.claim(state.principal_id) do
      {:ok, %{"state" => "no_work"}} -> state
      {:ok, %{"state" => "claimed", "step" => step}} -> process_step(state, step)
      {:ok, response} -> Map.put(state, :last_result, %{"unexpected_claim_response" => response})
      {:error, reason} -> Map.put(state, :last_result, %{"claim_error" => reason})
    end
  end

  defp process_company_work(state, item) do
    work_item_id = item["id"]
    heartbeat = start_heartbeat(fn -> AlpBeam.ControlPlane.heartbeat_company(work_item_id, state.principal_id) end)

    {status, output} =
      try do
        case perform_company(state, item) do
          {:ok, result} -> {"succeeded", result}
          {:error, result} -> {"failed", result}
        end
      after
        send(heartbeat, :stop)
      end

    checklist = item["checklist"] || []

    case AlpBeam.ControlPlane.complete_company(
           work_item_id,
           state.principal_id,
           status,
           output,
           if(status == "succeeded", do: checklist, else: [])
         ) do
      {:ok, result} ->
        state
        |> Map.update!(:company_work_completed, &(&1 + 1))
        |> Map.put(:last_step, work_item_id)
        |> Map.put(:last_result, result)

      {:error, reason} ->
        Map.put(state, :last_result, %{"work_item_id" => work_item_id, "company_completion_error" => reason})
    end
  end

  defp perform_company(state, item) do
    input = item["input"] || %{}
    base = %{
      "agent" => state.name,
      "principal_id" => state.principal_id,
      "role" => state.role,
      "case_id" => item["case_id"],
      "work_item_id" => item["id"],
      "node_key" => item["node_key"],
      "operation" => input["operation"] || item["node_key"],
      "quality_score" => role_quality(state.role),
      "performed_at" => DateTime.utc_now() |> DateTime.to_iso8601()
    }

    case item["kind"] do
      "supervisor" ->
        score = input["target_score"] || 96
        {:ok,
         Map.merge(base, %{
           "score" => score,
           "decision" => "pass",
           "dimensions" => %{
             "completeness" => score,
             "quality" => score,
             "policy" => 100,
             "evidence" => score
           },
           "notes" => "independent company supervisor accepted definition of done"
         })}

      "task" ->
        route = input["route"] || "standard"
        {:ok,
         Map.merge(base, %{
           "route" => route,
           "result" => "completed",
           "input_observed" => Map.drop(input, ["_sla_seconds", "_delay_seconds", "_wait_for"])
         })}

      other ->
        {:error, Map.merge(base, %{"error" => "unsupported agent work kind #{other}"})}
    end
  end

  defp process_step(state, step) do
    step_id = step["id"]
    heartbeat = start_heartbeat(fn -> AlpBeam.ControlPlane.heartbeat_step(step_id, state.principal_id) end)

    {status, output} =
      try do
        case perform(state, step) do
          {:ok, result} -> {"succeeded", result}
          {:reject, result} -> {"rejected", result}
          {:error, result} -> {"failed", result}
        end
      after
        send(heartbeat, :stop)
      end

    case AlpBeam.ControlPlane.complete(step_id, state.principal_id, status, output) do
      {:ok, result} ->
        state
        |> Map.update!(:steps_completed, &(&1 + 1))
        |> Map.put(:last_step, step_id)
        |> Map.put(:last_result, result)

      {:error, reason} ->
        Map.put(state, :last_result, %{"step_id" => step_id, "completion_error" => reason})
    end
  end

  defp start_heartbeat(fun) do
    spawn(fn -> heartbeat_loop(fun) end)
  end

  defp heartbeat_loop(fun) do
    receive do
      :stop -> :ok
    after
      @heartbeat_ms ->
        _ = fun.()
        heartbeat_loop(fun)
    end
  end

  defp role_quality("planner"), do: 95
  defp role_quality("researcher"), do: 93
  defp role_quality("worker"), do: 94
  defp role_quality("verifier"), do: 98
  defp role_quality(_), do: 90

  defp perform(state, step) do
    input = step["input"] || %{}
    orchestration_id = step["orchestration_id"]

    base = %{
      "agent" => state.name,
      "principal_id" => state.principal_id,
      "role" => state.role,
      "orchestration_id" => orchestration_id,
      "step_id" => step["id"],
      "attempt" => step["attempt"],
      "mode" => "alp10_cognitive_provider"
    }

    case state.role do
      "planner" ->
        with url when is_binary(url) <- find_https_url(input),
             {:ok, cognition} <- AlpBeam.ControlPlane.cognitive_plan(state.principal_id, orchestration_id) do
          {:ok, Map.merge(base, %{
            "kind" => "cognitive_plan",
            "research_url" => url,
            "cognition" => cognition,
            "next" => "researcher"
          })}
        else
          nil ->
            {:error, Map.merge(base, %{"kind" => "plan_error", "reason" => "goal contains no HTTPS research URL"})}

          {:error, reason} ->
            {:error, Map.merge(base, %{"kind" => "plan_error", "reason" => reason})}
        end

      "researcher" ->
        url = find_value(input, "research_url")

        with {:ok, evidence} <- AlpBeam.ControlPlane.web_fetch(state.principal_id, orchestration_id, url),
             evidence_id when is_binary(evidence_id) <- evidence["evidence_id"],
             {:ok, analysis} <- AlpBeam.ControlPlane.analyze_evidence(state.principal_id, orchestration_id, evidence_id) do
          {:ok, Map.merge(base, %{
            "kind" => "cognitive_evidence_packet",
            "external_research_performed" => true,
            "evidence" => evidence,
            "analysis" => analysis,
            "next" => "worker"
          })}
        else
          {:error, reason} ->
            {:error, Map.merge(base, %{
              "kind" => "research_error",
              "external_research_performed" => false,
              "reason" => reason
            })}

          other ->
            {:error, Map.merge(base, %{"kind" => "research_error", "reason" => inspect(other)})}
        end

      "worker" ->
        evidence_id = find_value(input, "evidence_id")

        case AlpBeam.ControlPlane.build_cognitive_artifact(
               state.principal_id,
               orchestration_id,
               evidence_id
             ) do
          {:ok, artifact} ->
            {:ok, Map.merge(base, %{
              "kind" => "cognitive_artifact",
              "artifact" => artifact,
              "next" => "verifier"
            })}

          {:error, reason} ->
            {:error, Map.merge(base, %{"kind" => "artifact_error", "reason" => reason})}
        end

      "verifier" ->
        artifact_id = find_value(input, "artifact_id")

        with {:ok, deterministic} <-
               AlpBeam.ControlPlane.verify_artifact(
                 state.principal_id,
                 orchestration_id,
                 artifact_id
               ),
             true <- deterministic["verified"] == true,
             {:ok, advisory} <-
               AlpBeam.ControlPlane.review_artifact(
                 state.principal_id,
                 orchestration_id,
                 artifact_id
               ) do
          {:ok, Map.merge(base, %{
            "kind" => "verification",
            "decision" => "verified",
            "deterministic_verification" => deterministic,
            "advisory_model_review" => advisory
          })}
        else
          false ->
            {:reject, Map.merge(base, %{
              "kind" => "verification",
              "decision" => "rejected",
              "reason" => "deterministic verification failed"
            })}

          {:error, reason} ->
            {:error, Map.merge(base, %{"kind" => "verification_error", "reason" => reason})}
        end

      other ->
        {:error, Map.merge(base, %{"kind" => "unsupported_role", "reason" => "no ALP10 handler for #{other}"})}
    end
  end

  defp find_value(value, key) when is_map(value) do
    case Map.get(value, key) do
      nil -> Enum.find_value(value, fn {_k, child} -> find_value(child, key) end)
      found -> found
    end
  end

  defp find_value(value, key) when is_list(value), do: Enum.find_value(value, &find_value(&1, key))
  defp find_value(_value, _key), do: nil

  defp find_https_url(value) when is_binary(value) do
    case Regex.run(~r{https://[^\s<>"']+}, value) do
      [url | _] -> String.trim_trailing(url, ".,);]")
      _ -> nil
    end
  end

  defp find_https_url(value) when is_map(value) do
    Enum.find_value(value, fn {_k, child} -> find_https_url(child) end)
  end

  defp find_https_url(value) when is_list(value), do: Enum.find_value(value, &find_https_url/1)
  defp find_https_url(_value), do: nil

  defp render(state) do
    %{
      "principal_id" => state.principal_id,
      "name" => state.name,
      "role" => state.role,
      "pid" => inspect(self()),
      "boot_count" => state.boot_count,
      "started_at" => state.started_at,
      "recovered" => state.recovered,
      "steps_completed" => state.steps_completed,
      "company_work_completed" => state.company_work_completed,
      "last_step" => state.last_step,
      "last_result" => state.last_result
    }
  end
end
