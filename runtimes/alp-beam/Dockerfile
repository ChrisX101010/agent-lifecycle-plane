FROM elixir:1.18-slim

WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

ENV MIX_ENV=prod

COPY mix.exs .
COPY lib ./lib

RUN elixir -e ':json.encode(%{"ok" => true}) |> IO.iodata_to_binary() |> IO.puts()'
RUN mix compile

RUN groupadd -r alp && useradd -r -g alp -u 10002 -m alp && chown -R alp:alp /app
USER alp

EXPOSE 8090
CMD ["mix", "run", "--no-halt"]
