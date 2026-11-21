# Plan: chips y portapapeles en el TUI nuevo

Este documento congela el acuerdo antes de tocar código.

## Decisiones

- Preview de texto: 6 caracteres.
- Chip de texto solo si el paste supera 32 caracteres.
- Imagen: `[Image #n]`.
- Archivo: `[File #n]`.
- Audio y video, si llegaran a darse: `[Audio #n]`, `[Video #n]`.
- Sin comando `/attach`.
- Sin drag and drop.
- Modelo desconocido o MIME no soportado: solo notice, sin chip, sin upload, sin mutación del `Draft`.
- Portapapeles único: el del sistema.
- `Ctrl+C`: copia si hay selección o chip bajo el cursor; si no, cancela el turno o sale como hoy.
- `Ctrl+X`: corta al portapapeles del sistema y elimina el segmento completo.
- `Ctrl+V`: lee siempre el portapapeles del sistema. El buffer interno del `Input` queda fuera de uso en el composer.

## Principios

- El chip es solo representación del composer. Lo que se envía es el contenido original.
- `Draft` es la única fuente de verdad. `Input.state.value` se deriva de `Draft::display()`.
- El chip se crea solo después de validar MIME y capabilities.
- Un chip se borra completo.
- No se añaden nodos focusables al composer.
- No se cambia el wire. Los marcadores van dentro del texto enviado.

## Arquitectura

### Dominio de chips

- `norte-code-tui/src/chip/label.rs`: `ChipLabel`.
- `norte-code-tui/src/chip/content.rs`: `ChipContent`.
- `norte-code-tui/src/chip/chip.rs`: `Chip`, `ChipId`, `ChipKind`.
- `norte-code-tui/src/chip/draft.rs`: `Draft`, `Segment`, `ChipSpan`.

### Portapapeles

- `norte-code-tui/src/clipboard/content.rs`: `ClipboardContent`, `ClipboardFile`.
- `norte-code-tui/src/clipboard/source.rs`: trait `ClipboardSource`.
- `norte-code-tui/src/clipboard/sink.rs`: trait `ClipboardSink`.
- `norte-code-tui/src/clipboard/system.rs`: adaptador con `arboard`.
- `norte-code-tui/src/clipboard/image.rs`: píxeles RGBA a PNG con `png`.
- `norte-code-tui/src/clipboard/notice.rs`: `ClipboardNotice`.

Dependencias nuevas: `arboard` y `png`.

### Adjuntos

- `norte-code-tui/src/attachment/mime.rs`: `MimeType`, detección por extensión.
- `norte-code-tui/src/attachment/kind.rs`: `AttachmentKind`, mapeo a `InputPartKind` y `Modality`.
- `norte-code-tui/src/attachment/capability.rs`: validación contra modelo activo y capabilities del servidor.
- `norte-code-tui/src/attachment/notice.rs`: `AttachmentNotice`.
- `norte-code-tui/src/attachment/error.rs`: errores tipados.

### UI y app

- `src/ui/composer.rs`: intercepta `Ctrl+C`, `Ctrl+X`, `Ctrl+V` y `InputEvent::Paste`.
- `src/app.rs`: enrutamiento condicional de `Ctrl+C`, comandos de upload, notices y errores de envío.
- `src/client.rs`: `UiCommand::Send` con parts, `UiCommand::Upload`, eventos de upload y `SendFailed`.
- `src/ui_model.rs`: `active_model_info()`.

## Flujo del composer

`Draft` guarda segmentos de texto y chips. `Draft::display()` concatenan labels y se mete en `Input.state.value`. `Draft::spans()` marca los rangos de caracteres ocupados por chips.

- Cursor y ancla se traducen entre índices del `Input` y segmentos del `Draft`.
- Si el cursor cae dentro de un chip, se ajusta al final del chip.
- Borrar un chip elimina el segmento completo.
- Copiar un chip de texto copia el contenido original.
- Copiar un chip de imagen o archivo copia su marcador real.
- `Input.clipboard` no se usa en el composer.

## Flujo de paste

`Ctrl+V`:

1. Lee el portapapeles del sistema.
2. Si hay imagen, valida MIME y capability del modelo activo.
3. Si hay archivos, valida cada uno por separado.
4. Si hay texto:
   - 32 caracteres o menos, inserción normal.
   - Más de 32, chip `[Copied: <6 chars>...]`.
5. Si el portapapeles no está disponible, notice de error.

Bracketed paste:

- Se mantiene como entrada de texto.
- Si el portapapeles del sistema contiene imagen o archivos, esos tienen prioridad.
- Si no, se usa el texto del evento.

## Flujo de attachment

Para imagen o archivo:

1. MIME siguiendo la validación real de RAEP: `image/*`, `audio/*`, `video/*`, `application/pdf`, `text/plain`, `text/markdown`.
2. `application/json` queda fuera por el desajuste client host.
3. Se comprueba que el servidor anuncie `uploads`.
4. Se resuelve el modelo activo con `UiModel::active_model_info()`.
5. Se comprueba `ModelInfo.input`.
6. Si algo falla: notice y nada más.
7. Si pasa: se suben bytes con `RestClient::upload`.
8. Solo tras recibir `UploadReceipt` se crea el chip.

Límites: 16 MiB por archivo.

## Envío e historial

- `UiCommand::Send` pasa `text` y `parts`.
- `Draft::expanded_text()` produce texto original, `[Image #n]`, `[File #n]` y texto escrito a mano.
- `Draft::wire_parts()` produce solo uploads.
- El mensaje optimista del transcript usa el mismo `expanded_text()`.
- `client.rs::send_prompt()` deja de descartar el `Result` y emite `ClientEvent::SendFailed`.
- RAEP replay solo guarda texto, así que los marcadores deben viajar dentro del texto.

## Copia global

- La copia del menú de mensajes y la selección de transcript usan el mismo `ClipboardSink`.
- Se eliminan las llamadas a `terminalui::osc52_write`.

## Avisos

- `AttachmentNotice` como enum de dominio.
- Helper que produce mensajes cortos.
- App convierte a `Item::Notice` con `Tone::Warn` o `Tone::Error`.
- Se corrige el desajuste entre `item_height()` y `paint_notice()`.

## Tests

- Ctrl+C con selección copia al sistema.
- Ctrl+C sin selección cancela o sale.
- Ctrl+X corta al sistema y elimina el chip completo.
- Ctrl+V lee del sistema y no del buffer interno del `Input`.
- Copiar un mensaje del transcript usa el mismo sink.
- Paste de 32 caracteres no crea chip.
- Paste de 33 caracteres crea chip con preview de 6 caracteres.
- Imagen del portapapeles crea `[Image #1]` solo si el modelo la acepta.
- Archivo del portapapeles crea `[File #1]`.
- MIME unsupported genera notice y deja el `Draft` intacto.
- Modelo desconocido genera notice y no crea chip.
- Fallo de upload no deja chip huérfano.
- Fallo de envío emite `SendFailed`.
- El texto enviado contiene contenido original y marcadores, nunca `[Copied: ...]`.
- El historial replay conserva `[Image #n]` y `[File #n]`.

## Orden de implementación

1. Portapapeles, traits y adaptador con tests.
2. Dominio de chips y `Draft`.
3. Integración del composer con paste, copy, cut y borrado atómico.
4. Capability gate y MIME.
5. Upload commands y eventos.
6. Send con parts y propagación de errores.
7. Notices y corrección de altura.
8. `cargo fmt`, `clippy -D warnings`, tests, install y CI.
