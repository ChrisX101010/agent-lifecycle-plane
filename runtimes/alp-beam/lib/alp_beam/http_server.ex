defmodule AlpBeam.HttpServer do
  use GenServer
  require Logger

  def start_link(_opts), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)

  @impl true
  def init(state) do
    port =
      System.get_env("ALP_BEAM_PORT", "8090")
      |> String.to_integer()

    {:ok, listen} =
      :gen_tcp.listen(port, [
        :binary,
        packet: :raw,
        active: false,
        reuseaddr: true
      ])

    Logger.info("ALP BEAM runtime listening on 0.0.0.0:#{port}")
    send(self(), :accept)
    {:ok, Map.put(state, :listen, listen)}
  end

  @impl true
  def handle_info(:accept, %{listen: listen} = state) do
    case :gen_tcp.accept(listen, 1000) do
      {:ok, socket} ->
        Task.start(fn -> serve(socket) end)
        send(self(), :accept)

      {:error, :timeout} ->
        send(self(), :accept)

      {:error, reason} ->
        Logger.error("BEAM runtime accept error: #{inspect(reason)}")
        Process.send_after(self(), :accept, 250)
    end

    {:noreply, state}
  end

  defp serve(socket) do
    {status, body} =
      case :gen_tcp.recv(socket, 0, 3_000) do
        {:ok, request} -> route(request)
        {:error, reason} -> {500, %{"error" => inspect(reason)}}
      end

    encoded = IO.iodata_to_binary(:json.encode(body))

    packet = [
      "HTTP/1.1 #{status} #{status_text(status)}\r\n",
      "content-type: application/json\r\n",
      "content-length: #{byte_size(encoded)}\r\n",
      "connection: close\r\n",
      "\r\n",
      encoded
    ]

    :gen_tcp.send(socket, packet)
    :gen_tcp.close(socket)
  end

  defp route(request) do
    [request_line | _] = String.split(request, "\r\n")

    case String.split(request_line, " ") do
      ["GET", "/health", _] ->
        {200,
         %{
           "status" => "ok",
           "service" => "alp-beam-runtime",
           "runtime" => "beam",
           "otp_release" => to_string(:erlang.system_info(:otp_release))
         }}

      ["GET", "/v1/runtime/tree", _] ->
        {200, AlpBeam.Runtime.tree()}

      ["GET", "/v1/agents", _] ->
        {200, %{"agents" => AlpBeam.Runtime.list_agents()}}

      ["GET", path, _] ->
        with {:ok, principal_id} <- agent_path(path),
             {:ok, info} <- AlpBeam.Runtime.agent_info(principal_id) do
          {200, info}
        else
          _ -> {404, %{"error" => "not_found"}}
        end

      ["POST", path, _] ->
        post_route(path)

      _ ->
        {404, %{"error" => "not_found"}}
    end
  rescue
    error -> {500, %{"error" => Exception.message(error)}}
  end

  defp post_route(path) do
    cond do
      String.ends_with?(path, "/crash") ->
        principal_id = path |> String.trim_trailing("/crash") |> last_segment()

        case AlpBeam.Runtime.crash(principal_id) do
          :ok -> {202, %{"principal_id" => principal_id, "state" => "crash_requested"}}
          _ -> {404, %{"error" => "agent_not_found"}}
        end

      String.ends_with?(path, "/recover") ->
        principal_id = path |> String.trim_trailing("/recover") |> last_segment()

        case AlpBeam.Runtime.recover(principal_id) do
          {:ok, info} -> {200, info}
          _ -> {404, %{"error" => "agent_not_found"}}
        end

      true ->
        {404, %{"error" => "not_found"}}
    end
  end

  defp agent_path("/v1/agents/" <> principal_id) when principal_id != "",
    do: {:ok, principal_id}

  defp agent_path(_), do: :error

  defp last_segment(path) do
    path
    |> String.split("/", trim: true)
    |> List.last()
  end

  defp status_text(200), do: "OK"
  defp status_text(202), do: "Accepted"
  defp status_text(404), do: "Not Found"
  defp status_text(500), do: "Internal Server Error"
  defp status_text(_), do: "OK"
end
