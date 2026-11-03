defmodule AlpBeam.Runtime do
  def list_agents do
    DynamicSupervisor.which_children(AlpBeam.AgentSupervisor)
    |> Enum.flat_map(fn
      {_id, pid, :worker, _modules} when is_pid(pid) ->
        case safe_call(pid, :info) do
          {:ok, info} -> [info]
          _ -> []
        end

      _ ->
        []
    end)
    |> Enum.sort_by(& &1["name"])
  end

  def agent_info(principal_id) do
    with [{pid, _}] <- Registry.lookup(AlpBeam.ProcessRegistry, principal_id),
         {:ok, info} <- safe_call(pid, :info) do
      {:ok, info}
    else
      [] -> {:error, :not_found}
      {:error, reason} -> {:error, reason}
    end
  end

  def recover(principal_id) do
    with [{pid, _}] <- Registry.lookup(AlpBeam.ProcessRegistry, principal_id),
         {:ok, info} <- safe_call(pid, :recover) do
      {:ok, info}
    else
      [] -> {:error, :not_found}
      {:error, reason} -> {:error, reason}
    end
  end

  def crash(principal_id) do
    case Registry.lookup(AlpBeam.ProcessRegistry, principal_id) do
      [{pid, _}] ->
        GenServer.cast(pid, :crash)
        :ok

      [] ->
        {:error, :not_found}
    end
  end

  def tree do
    %{
      "supervisor_pid" => inspect(Process.whereis(AlpBeam.Supervisor)),
      "agent_supervisor_pid" => inspect(Process.whereis(AlpBeam.AgentSupervisor)),
      "agent_count" => length(list_agents()),
      "agents" => list_agents(),
      "boot_counts" => AlpBeam.RuntimeState.snapshot()
    }
  end

  defp safe_call(pid, message) do
    try do
      {:ok, GenServer.call(pid, message, 5_000)}
    catch
      :exit, reason -> {:error, reason}
    end
  end
end
