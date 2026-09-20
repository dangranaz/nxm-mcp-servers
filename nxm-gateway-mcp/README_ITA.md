# nxm-gateway-mcp

Un **gateway MCP federato thin**. Espone un unico endpoint MCP all'agente e
propaga (fan-out) ogni query ai **nodi DB di nxm-memory** configurati, fondendone
i risultati con **Reciprocal Rank Fusion (RRF)**.

**Non contiene index e non carica modelli** — è un proxy stateless. Il lavoro
pesante (indicizzazione, embedding, ricerca) avviene nei nodi DB con cui parla.

> README bilingue — inglese: [`README_ENG.md`](./README_ENG.md).

## Cosa fa

- **Ricerca federata** (`index_search`, `search_code`, `search_docs`,
  `search_exact`, `search_regex`, `memory_recall`): interroga ogni nodo in
  parallelo, fonde con RRF, restituisce i migliori risultati con il loro
  `source_node`.
- **Fan-out-first** (`get_chunk`): sonda tutti i nodi, restituisce il primo che
  ha l'elemento.
- **Passthrough** (`context_compress`, `context_budget`, `find_symbol`,
  `outline`, `find_references`, `stats`, `memory_remember`, `index_workspace`,
  `workspace_list`, `workspace_create`): inoltra a un nodo (instradato da un
  argomento opzionale `workspace`, altrimenti il primo nodo).

## Configurazione

I nodi vengono risolti all'avvio, con questo ordine di precedenza:

1. Variabile d'ambiente `NXM_GATEWAY_NODES` — `nome:porta,nome:porta,...`
2. `~/.nxm/memory/config.toml`, sezione `[gateway].nodes`

```toml
[gateway]
[[gateway.nodes]]
name = "engine"
port = 7169
enabled = true
```

## Avvio

```bash
nxm-gateway-mcp   # JSON-RPC su stdio
```

Configuralo in un agente compatibile MCP:

```json
{
  "mcpServers": {
    "nxm-gateway": {
      "command": "nxm-gateway-mcp",
      "env": { "NXM_GATEWAY_NODES": "engine:7169,vendors:7173" }
    }
  }
}
```

## Licenza

Dual-license MIT OR Apache-2.0.
