defmodule AlpBeam.MixProject do
  use Mix.Project

  def project do
    [
      app: :alp_beam,
      version: "0.1.0",
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      deps: []
    ]
  end

  def application do
    [
      extra_applications: [:logger],
      mod: {AlpBeam.Application, []}
    ]
  end
end
