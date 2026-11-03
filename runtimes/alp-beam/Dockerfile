FROM elixir:1.18-slim

WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

ENV MIX_ENV=prod

COPY mix.exs .
COPY lib ./lib

RUN elixir -e ':json.encode(%{"ok" => true}) |> IO.iodata_to_binary() |> IO.puts()'
RUN mix compile

EXPOSE 8090
CMD ["mix", "run", "--no-halt"]
