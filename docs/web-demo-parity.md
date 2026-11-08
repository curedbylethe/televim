# Web demo parity matrix

This matrix tracks current native controls against the browser demo. **simulated** means real Rust state and deterministic local behavior. **browser-adapted** means real Rust behavior uses a browser capability or shell path. **impossible** means the browser has no honest equivalent. **native-missing** means documentation names a feature absent from the current TUI.

Browser checks below are manual until Stage 11 adds Playwright. Stage 08 adds secondary flows; Stage 10 adds Unicode and bidi evidence.

| Area | Flow / key | Native trigger | Demo behavior | Category | Evidence |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Modes | Normal | `Esc` from Insert or Visual | Real `Mode::Normal` | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Modes | Insert | `i` | Real composer mode | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Modes | Visual | `v` | Real character selection mode | simulated | `parity_core::selection_yank_and_quit_set_native_state` |
| Modes | Linewise Visual | `V` | Real line selection mode | manual: browser core modes | browser manual: core modes |
| Counts | Counted motion | `3j`, `3k`, `2w` | Native pending count and motion | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Motions | Chat message | `j`, `k`, `h`, `l`, `w`, `b`, `e`, `0`, `$`, `gg`, `G` | Native conversation cursor | manual: browser core motions | browser manual: core motions |
| Motions | Page | `Ctrl-d`, `Ctrl-u`, `PageUp`, `PageDown` | Native conversation window paging | manual: browser core paging | browser manual: core paging |
| Motions | Jump history | `Ctrl-o`, `Ctrl-i` | Native jump list | manual: browser jump history |
| Operators | Delete or yank with motion | `d` / `y` + motion, `dd`, `yy` | Native operator state and action | simulated | `parity_core::invalid_count_clears_and_visual_delete_asks_for_confirmation`; `parity_core::selection_yank_and_quit_set_native_state` |
| Focus | Cycle panes | `Tab`, `BackTab` | Native pane focus | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Focus | Conversation or profile | `h`, `l`, `Enter` | Native chat and profile navigation | manual: browser focus and cards | browser manual: focus and cards |
| Prompt | Open command prompt | `:` | Native command prompt | manual: browser prompt | browser manual: command prompt |
| Prompt | Search chats | `?` | Native chat search | manual: browser prompt | browser manual: chat search |
| Prompt | Search current chat | `/`, `n`, `N` | Native message search | manual: browser prompt | browser manual: message search |
| Scrolling | Follow and jump | `gg`, `G`, `Ctrl-d`, `Ctrl-u` | Native conversation window | manual: browser scrolling | browser manual: scrolling |
| Scrolling | Mouse wheel | Mouse wheel | Browser scroll input reaches the focused terminal | browser-adapted | browser manual: mouse scrolling |
| Selection | Character selection and yank | `v`, motion, `y` | Native selection; yank text goes through clipboard bridge | browser-adapted | `parity_core::selection_yank_and_quit_set_native_state`; browser manual: clipboard |
| Selection | Line selection | `V`, motion, `y` | Native line selection and clipboard bridge | browser-adapted | browser manual: line selection and clipboard |
| Confirm | Quit conversation | `q`, then `y` / `n` / `Esc` | Native confirmation; `y` routes to shell quit event | browser-adapted | browser manual: quit and reset |
| Confirm | Delete messages | `d`, then `y` / `n` / `Esc` | Native confirmation and local simulated delete | simulated | `parity_core::invalid_count_clears_and_visual_delete_asks_for_confirmation`; `parity_core::send_reply_edit_delete_and_rename_use_fixture_state` |
| Confirm | Discard draft | `:discard`, then confirm | Native confirmation | simulated | `parity_core::discard_command_confirms_and_clears_the_draft` |
| Drafts | Edit and switch chats | `i`, type, `Esc`, switch away and back | Native in-memory draft store | simulated | `parity_core::emoji_completion_and_draft_survive_chat_switch` |
| Drafts | Reset | Reset button | New demo world clears drafts and local changes | browser-adapted | browser manual: reset clears state |
| Emoji | Complete shortcode | Type `:smile`, `Tab` or `Enter` | Native popup inserts emoji into composer | simulated | `parity_core::emoji_completion_and_draft_survive_chat_switch` |
| Emoji | Filter and dismiss | Type shortcode, `Up` / `Down` / `Esc` | Native popup filtering and dismissal | manual: browser emoji popup |
| Rename | Set or clear contact name | `:rename` | Native local name action updates chat and profile | simulated | `parity_core::send_reply_edit_delete_and_rename_use_fixture_state` |
| Messaging | Send | `Enter` | Native optimistic send and deterministic simulator result | simulated | `parity_core::send_reply_edit_delete_and_rename_use_fixture_state`; `parity_core::send_failure_dismiss_and_attachment_retry_follow_native_actions` |
| Messaging | Edit, delete, reply | `e`, `d`, `r` | Native message operations with local simulator effects | simulated | `parity_core::send_reply_edit_delete_and_rename_use_fixture_state` |
| Messaging | Failed send, retry upload, dismiss | `o`, `D` | Native failure state and upload actions | simulated | `parity_core::send_failure_dismiss_and_attachment_retry_follow_native_actions` |
| Quit | Immediate quit | `Ctrl-C` or `:q` | Real `App` sets quit state; driver emits `televim-demo-quit` for shell release | browser-adapted | `parity_core::selection_yank_and_quit_set_native_state`; browser manual: quit and reset |
| Quit | Close browser tab | `Ctrl-W` | Native key still reaches `App`; prevent default while terminal has focus | browser-adapted | browser manual: Ctrl-W with terminal focused |
| Update | Install update | `:update` | “Updates are disabled in the offline demo.” | impossible | browser manual: update refusal |

## Source inventory

| Source | Inventory result |
| :--- | :--- |
| `README.md` essential controls and command list | Core controls above; chat actions, contacts, archive and search details continue in Stage 08. |
| `docs/product.md` feature set | Messaging, visual selection, emoji, local names, and update flow are represented above; profile cards, media, presence, retries, search breadth, and account management continue in Stage 08. |
| `crates/tui/src/state/coordinate.rs` | Mode, focus, prompt, confirm, motion, selection, and operator dispatch are represented above. Code is authoritative where README wording differs. |
| `crates/tui/src/line.rs` | Composer editing, shortcode completion, and input keys are represented above. |
| `crates/tui/src/card.rs` | Profile card navigation and rename are represented above; remaining card actions continue in Stage 08. |
| `crates/tui/src/state/update.rs` | Update overlay is native. Installing cannot run in the offline browser demo and is marked impossible above. |

## Browser manual checks

- **core modes:** Enter Insert, return with `Esc`, enter Visual, and verify mode indicators.
- **core motions:** Try counts, word motions, paging, first/last, and jump history.
- **operators:** Yank and delete selected messages; confirm prompts respond to `y`, `n`, and `Esc`.
- **focus and cards:** Cycle focus and open a contact card; rename, switch chats, and verify the alias persists.
- **prompt:** Open `:` and `/`, accept and cancel prompts, then try an invalid command.
- **scrolling:** Use keys and mouse wheel; verify auto-follow and jumps.
- **clipboard:** Deny clipboard access and verify demo reports fallback instead of success.
- **emoji popup:** Filter a shortcode, accept with `Tab` and `Enter`, dismiss with `Esc`.
- **draft and reset:** Switch chats with a draft, return, then Reset and verify state clears.
- **messaging:** Send, edit, reply, delete, fail, retry, and dismiss a local send.
- **quit and reset:** Use `:q` and `Ctrl-C`; shell handles `televim-demo-quit`, then Reset re-enters.
- **Ctrl-W:** Focus terminal, press `Ctrl-W`, verify tab remains and native line behavior occurs.
- **update refusal:** Open update flow and verify “Updates are disabled in the offline demo.”
