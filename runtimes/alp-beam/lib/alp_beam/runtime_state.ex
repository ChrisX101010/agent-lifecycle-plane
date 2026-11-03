defmodule AlpBeam.RuntimeState do
  use GenServer

  def start_link(_opts), do: GenServer.start_link(__MODULE__, %{}, name: __MODULE__)

  def next_boot(principal_id), do: GenServer.call(__MODULE__, {:next_boot, principal_id})
  def snapshot, do: GenServer.call(__MODULE__, :snapshot)

  @impl true
  def init(state), do: {:ok, state}

  @impl true
  def handle_call({:next_boot, principal_id}, _from, state) do
    boot = Map.get(state, principal_id, 0) + 1
    {:reply, boot, Map.put(state, principal_id, boot)}
  end

  @impl true
  def handle_call(:snapshot, _from, state), do: {:reply, state, state}
end
