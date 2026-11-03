defmodule AlpBeam.Application do
  use Application

  @impl true
  def start(_type, _args) do
    children = [
      {Registry, keys: :unique, name: AlpBeam.ProcessRegistry},
      AlpBeam.RuntimeState,
      {DynamicSupervisor, strategy: :one_for_one, name: AlpBeam.AgentSupervisor},
      AlpBeam.Bootstrap,
      AlpBeam.HttpServer
    ]

    Supervisor.start_link(children, strategy: :one_for_one, name: AlpBeam.Supervisor)
  end
end
