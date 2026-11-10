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

norte-code-client → wire, raep-types (decodifica el protocolo; el adaptador
                    HTTP nativo vive detrás de la feature `native`)
norte-code-agent  → protocol, wire
norte-code-raep   → agent, protocol, wire, plugin-host
norte-code-app    → client (sin features por defecto), wire, raep-types
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

1. Avisos de cuota en UI, multimodal de entrada punta a punta, steering.
2. `norte-code-agent`: `a_failed_eager_summary_stops_at_the_safe_point` falla
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
