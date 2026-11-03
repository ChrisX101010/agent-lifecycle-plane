defmodule AlpBeam.ControlPlane do
  require Logger

  def recovery(principal_id) do
    case request("GET", "/v1/runtime/agents/#{principal_id}/recovery", nil) do
      {:ok, 200, body} -> body
      {:ok, status, body} -> %{"error" => "control_plane_http_#{status}", "body" => body}
      {:error, reason} -> %{"error" => "control_plane_unavailable", "reason" => inspect(reason)}
    end
  rescue
    error ->
      Logger.error("ALP recovery request failed: #{Exception.message(error)}")
      %{"error" => "control_plane_exception", "reason" => Exception.message(error)}
  end

  def claim_company(principal_id), do: post_ok("/v1/company/agents/#{principal_id}/claim", %{})

  def complete_company(work_item_id, principal_id, status, output, checklist_completed) do
    post_ok("/v1/company/work-items/#{work_item_id}/complete", %{
      "principal_id" => principal_id,
      "status" => status,
      "output" => output,
      "checklist_completed" => checklist_completed
    })
  end

  def heartbeat_company(work_item_id, principal_id) do
    post_ok("/v1/company/work-items/#{work_item_id}/heartbeat", %{
      "principal_id" => principal_id
    })
  end

  def claim(principal_id), do: post_ok("/v1/orchestration/agents/#{principal_id}/claim", %{})

  def heartbeat_step(step_id, principal_id) do
    post_ok("/v1/orchestration/steps/#{step_id}/heartbeat", %{
      "principal_id" => principal_id
    })
  end

  def complete(step_id, principal_id, status, output) do
    post_ok("/v1/orchestration/steps/#{step_id}/complete", %{
      "principal_id" => principal_id,
      "status" => status,
      "output" => output
    })
  end

  def web_fetch(principal_id, orchestration_id, url) do
    post_created("/v1/tools/web-fetch", %{
      "principal_id" => principal_id,
      "orchestration_id" => orchestration_id,
      "url" => url
    })
  end

  def verify_artifact(principal_id, orchestration_id, artifact_id) do
    post_ok("/v1/tools/artifacts/#{artifact_id}/verify", %{
      "principal_id" => principal_id,
      "orchestration_id" => orchestration_id
    })
  end

  def cognitive_plan(principal_id, orchestration_id) do
    post_ok("/v1/cognition/plan", %{
      "principal_id" => principal_id,
      "orchestration_id" => orchestration_id
    })
  end

  def analyze_evidence(principal_id, orchestration_id, evidence_id) do
    post_ok("/v1/cognition/evidence/#{evidence_id}/analyze", %{
      "principal_id" => principal_id,
      "orchestration_id" => orchestration_id
    })
  end

  def build_cognitive_artifact(principal_id, orchestration_id, evidence_id) do
    post_created("/v1/cognition/artifacts/build", %{
      "principal_id" => principal_id,
      "orchestration_id" => orchestration_id,
      "evidence_id" => evidence_id
    })
  end

  def review_artifact(principal_id, orchestration_id, artifact_id) do
    post_ok("/v1/cognition/artifacts/#{artifact_id}/review", %{
      "principal_id" => principal_id,
      "orchestration_id" => orchestration_id
    })
  end

  defp post_ok(path, body) do
    case request("POST", path, body) do
      {:ok, 200, response} -> {:ok, response}
      {:ok, status, response} -> {:error, %{"status" => status, "body" => response}}
      {:error, reason} -> {:error, %{"reason" => inspect(reason)}}
    end
  end

  defp post_created(path, body) do
    case request("POST", path, body) do
      {:ok, 201, response} -> {:ok, response}
      {:ok, status, response} -> {:error, %{"status" => status, "body" => response}}
      {:error, reason} -> {:error, %{"reason" => inspect(reason)}}
    end
  end

  defp request(method, path, body) do
    base = System.get_env("ALP_CONTROL_PLANE_URL", "http://api:8080") |> URI.parse()
    host = base.host || "api"
    port = base.port || 80

    with {:ok, socket} <- :gen_tcp.connect(String.to_charlist(host), port, [:binary, active: false, packet: :raw], 5_000),
         {:ok, request_body} <- encode_body(body),
         :ok <- send_request(socket, method, host, path, request_body),
         {:ok, raw} <- recv_all(socket, []),
         {:ok, status, response_body} <- parse_response(raw) do
      :gen_tcp.close(socket)
      {:ok, status, decode(response_body)}
    end
  end

  defp encode_body(nil), do: {:ok, ""}
  defp encode_body(body), do: {:ok, body |> :json.encode() |> IO.iodata_to_binary()}

  defp send_request(socket, method, host, path, body) do
    auth_header =
      case System.get_env("ALP_API_TOKEN", "") do
        "" -> []
        token -> ["Authorization: Bearer ", token, "\r\n"]
      end

    headers = [
      method, " ", path, " HTTP/1.1\r\n",
      "Host: ", host, "\r\n",
      "Accept: application/json\r\n",
      auth_header,
      "Connection: close\r\n"
    ]

    packet =
      if method == "POST" do
        [headers, "Content-Type: application/json\r\n", "Content-Length: ",
         Integer.to_string(byte_size(body)), "\r\n\r\n", body]
      else
        [headers, "\r\n"]
      end

    :gen_tcp.send(socket, packet)
  end

  defp recv_all(socket, acc) do
    case :gen_tcp.recv(socket, 0, 130_000) do
      {:ok, chunk} -> recv_all(socket, [chunk | acc])
      {:error, :closed} -> {:ok, acc |> Enum.reverse() |> IO.iodata_to_binary()}
      {:error, reason} -> {:error, reason}
    end
  end

  defp parse_response(raw) do
    case :binary.split(raw, "\r\n\r\n", [:global]) do
      [headers, body] ->
        [status_line | _] = String.split(headers, "\r\n")
        case String.split(status_line, " ", parts: 3) do
          [_http, status, _reason] ->
            case Integer.parse(status) do
              {code, ""} -> {:ok, code, body}
              _ -> {:error, :invalid_status}
            end
          _ -> {:error, :invalid_status_line}
        end
      _ -> {:error, :invalid_http_response}
    end
  end

  defp decode(""), do: %{}
  defp decode(body), do: :json.decode(body)
end
