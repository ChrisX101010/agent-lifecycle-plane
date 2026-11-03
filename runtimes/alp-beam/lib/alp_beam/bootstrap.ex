defmodule AlpBeam.Bootstrap do
  use GenServer
  require Logger

  def start_link(_opts), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)

  @impl true
  def init(state) do
    Process.send_after(self(), :bootstrap, 100)
    {:ok, state}
  end

  @impl true
  def handle_info(:bootstrap, state) do
    agents = parse_agents(System.get_env("ALP_BEAM_AGENTS", ""))

    Enum.each(agents, fn config ->
      case DynamicSupervisor.start_child(AlpBeam.AgentSupervisor, {AlpBeam.AgentProcess, config}) do
        {:ok, pid} ->
          Logger.info("BEAM agent started #{config.name} #{config.principal_id} pid=#{inspect(pid)}")

        {:error, {:already_started, pid}} ->
          Logger.info("BEAM agent already running #{config.name} pid=#{inspect(pid)}")

        {:error, reason} ->
          Logger.error("BEAM agent failed #{config.name}: #{inspect(reason)}")
      end
    end)

    {:noreply, Map.put(state, :agents, agents)}
  end

  defp parse_agents(raw) do
    raw
    |> String.split(";", trim: true)
    |> Enum.map(&String.trim/1)
    |> Enum.reject(&(&1 == ""))
    |> Enum.map(fn entry ->
      case String.split(entry, "|", parts: 3) do
        [principal_id, name, role] ->
          %{principal_id: principal_id, name: name, role: role}

        _ ->
          raise "invalid ALP_BEAM_AGENTS entry: #{entry}"
      end
    end)
  end
end
