# Plan — crate compartido de tipos de wire

## Arquitectura (estado actual)

```
norte-code-wire        serde + schemars + serde_json + base64. Sin socket, sin
                       runtime, sin base de datos. Compila a wasm.
  types.rs             tipos puros del protocolo (SessionInfo, JobInfo, Delta,
                       Usage, Budget, Reasoning, ChildState, MediaPart…)
  wire.rs              peticiones y eventos: ClientRequest(Kind), ServerEvent(Kind),
                       PendingModel/PendingOverride/OutputModality/ForkInclude…
  payload.rs           payloads canónicos de Norte (token/tool/job/question/
                       todos/child/agent/compaction/output-part/request-completed…)
  resource.rs          recursos HTTP: Session/Agent/Job/Plan/Quota/Telemetry/
                       ChildSummary, JobStatus, SessionStatus, RequestStatus
  event.rs             CanonicalEvent: enum cerrado de etiquetas `event`/`kind`

norte-code-protocol    protocol + wire: store SQLite, config, catálogo, costes,
                       skills, planes. Reexporta todos los tipos de wire, así que
                       `norte_code_protocol::Foo` sigue resolviendo.

norte-code-client → protocol
norte-code-agent  → client, protocol
norte-code-raep   → agent, protocol, wire, plugin-host
norte-code-cli    → client, protocol (usa los tipos de wire por el reexport)
norte-code-app    → wire, raep-types (sin tokio/rusqlite)
```

## Qué se centralizó

- **`protocol::wire` entero movido a `wire::wire`** (1400 líneas): peticiones,
  los ~60 `ServerEventKind` y los tipos de comando/override. Protocol solo
  reexporta.
- **Tipos puros** (`protocol::types`): definidos en `wire::types`, reexportados.
- **`QueueMode`, `PlanStatus`** movidos a wire.
- **`MediaPart`/`MediaSource`/`MediaMetadata`/`ProviderFileRef`**: espejo en wire
  (con el base64 del motor). El agente convierte al emitir
  (`service/media_wire.rs`) y el host consume el tipo del wire.
- **`TelemetryRecord`**: forma de transporte de la fila de sqlite, con
  conversiones `From<&RequestRecord>` / `TryFrom<TelemetryRecord>` en protocol.
- **Payloads canónicos**: el host dejó de tener structs privados; la app dejó de
  tener sus decodificadores gemelos.
- **Recursos HTTP**: `SessionSummary` (con el `Box::leak` eliminado),
  `ChildSessionSummary`, `AgentSummary`/`AgentCapabilities`/`AgentModalityLimits`
  (otros dos `Box::leak` eliminados), `JobStatus`/`JobSummary`/`JobFinished`,
  `PlanSummary`, `QuotaReport`, `TelemetryReport`, `ModelInfo`.
- **`CanonicalEvent`**: enum cerrado; ya no se comparan strings `event`/`kind`.

## Derivas que la unificación destapó y se corrigieron

- La app decodificaba `agent_changed` esperando `{provider, model}` y el host
  escribía `{from_agent, to_agent}`: todo cambio de modelo se ignoraba.
- La app llamaba `ApprovalResolved` a `QuestionAnswered`.
- El host escribía el tag `output_part_finished_wire` (nombre Rust filtrado al
  JSON); ahora es `output_part_finished`.

## Pendiente

1. `norte-code-app`: `ServerInfo`/`ProtocolInfo`/`ServerLimits`/`Health*`/
   `Readiness*`/`PluginSummary`/`SkillCatalog`/`ToolRule`/`McpCatalog`/
   `CostReport`/`UploadReceipt`/`CatalogDetail` siguen siendo copias locales;
   el host usa nombres distintos (`InfoPayload`, `LimitsRef`, `McpListData`…)
   para el mismo JSON. Siguiente tanda.
2. `wire_types.rs` aún declara `TurnState` y `HistoryPage/HistoryItem/History*`
   mientras el host serializa `raep_types::{TurnStateChanged, HistoryPage,
   HistoryItem}`; hay que decidir si la app usa `raep-types` para esas o si
   wire las modela.
3. `norte-code-cli` no declara `wire` directamente: consume los tipos por el
   reexport de protocol. Decidir si se ata a wire donde corresponda.
4. Funcionalidad de la tanda anterior: avisos de cuota en UI, multimodal de
   entrada punta a punta, steering.
5. `norte-code-agent`: `a_failed_eager_summary_stops_at_the_safe_point` falla
   esporádicamente bajo carga paralela (pasa aislado y 3 de 3 en serie).

## Verificación

- `cargo test --workspace`: 788 passing.
- `cargo clippy --workspace --all-targets -- -D warnings`: limpio.
- `cargo build -p norte-code-app --target wasm32-unknown-unknown`: OK.
- En vivo (servidor real, config temporal con `amazon/nova-lite-v1` porque
  `openrouter/stealth/crouton-alpha` está retirado): arnés `text` (30 deltas,
  COMPLETED) y `tools` (2/2 tools, 1 todo, COMPLETED); `/sessions`, `/agents`,
  `/quota` y `/telemetry` devuelven exactamente los campos de los structs
  compartidos.
