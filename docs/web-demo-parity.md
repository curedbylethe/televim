# Web demo parity matrix

This matrix tracks current native controls against the browser demo. **simulated** means real Rust state and deterministic local behavior. **browser-adapted** means real Rust behavior uses a browser capability or shell path. **impossible** means the browser has no honest equivalent, with its exact refusal text.

Playwright specs cover the shell's poster, lazy load, keyboard, Reset, focus
release, mobile input and IME, failure states, provenance, network boundary and
WebGL2 startup. The gated `wasm` CI job runs this suite after its browser Rust
tests pass. Local runs require Trunk and Playwright; browser-adapted rows also
list manual checks where needed.

| Browser check | Evidence | Local result |
| :--- | :--- | :--- |
| Poster, lazy module load, provenance, no external requests | [`shell.spec.ts`](../crates/web-demo/e2e/tests/shell.spec.ts) | Gated `wasm` CI; local run requires Trunk |
| Keyboard prompts, Insert, bare Esc, Reset and release | [`shell.spec.ts`](../crates/web-demo/e2e/tests/shell.spec.ts) | Gated `wasm` CI; local run requires Trunk |
| Mobile keys, text input and committed IME | [`shell.spec.ts`](../crates/web-demo/e2e/tests/shell.spec.ts) | Gated `wasm` CI; local run requires Trunk |
| Clipboard success and denial fallback | [`shell.spec.ts`](../crates/web-demo/e2e/tests/shell.spec.ts) | Gated `wasm` CI; local run requires Trunk |
| Unsupported, failed and WebGL2 states | [`shell.spec.ts`](../crates/web-demo/e2e/tests/shell.spec.ts) | Gated `wasm` CI; local run requires Trunk |

| Area | Flow / key | Native trigger | Demo behavior | Category | Evidence |
| :--- | :--- | :--- | :--- | :--- | :--- |
| Modes | Normal | `Esc` from Insert or Visual | Real `Mode::Normal` | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Modes | Insert | `i` | Real composer mode | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Modes | Visual | `v` | Real character selection mode | simulated | `parity_core::selection_yank_and_quit_set_native_state` |
| Modes | Linewise Visual | `V` | Real line selection mode | simulated | `parity_core::selection_yank_and_quit_set_native_state`; browser manual: core modes |
| Counts | Counted motion | `3j`, `3k`, `2w` | Native pending count and motion | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Motions | Chat message | `j`, `k`, `h`, `l`, `w`, `b`, `e`, `0`, `$`, `gg`, `G` | Native conversation cursor | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers`; browser manual: core motions |
| Motions | Page | `Ctrl-d`, `Ctrl-u`, `PageUp`, `PageDown` | Native conversation window paging | simulated | `tui::app::tests::a_page_moves_a_screenful_and_the_bottom_re_engages_following`; browser manual: core paging |
| Motions | Jump history | `Ctrl-o`, `Ctrl-i` | Native jump list | simulated | `tui::jumplist::tests::the_first_walk_back_marks_where_the_reader_was`; browser manual: jump history |
| Operators | Delete or yank with motion | `d` / `y` + motion, `dd`, `yy` | Native operator state and action | simulated | `parity_core::invalid_count_clears_and_visual_delete_asks_for_confirmation`; `parity_core::selection_yank_and_quit_set_native_state` |
| Focus | Cycle panes | `Tab`, `BackTab` | Native pane focus | simulated | `parity_core::modes_counts_motion_and_focus_use_native_handlers` |
| Focus | Conversation or profile | `h`, `l`, `Enter` | Native chat and profile navigation | simulated | `tui::app::tests::h_leaves_the_conversation_for_the_chat_list_and_l_comes_back`; browser manual: focus and cards |
| Prompt | Open command prompt | `:` | Native command prompt | simulated | `tui::app::tests::opening_the_command_prompt_keeps_the_draft`; browser manual: command prompt |
| Prompt | Search chats | `?` | Native global search | simulated | `tui::app::tests::the_global_prompt_is_titled_search`; browser manual: chat search |
| Prompt | Search current chat | `/`, `n`, `N` | Native message search | simulated | `tui::app::tests::a_slash_on_the_conversation_still_opens_the_message_search`; browser manual: message search |
| Scrolling | Follow and jump | `gg`, `G`, `Ctrl-d`, `Ctrl-u` | Native conversation window | simulated | `tui::app::tests::a_page_moves_a_screenful_and_the_bottom_re_engages_following`; browser manual: scrolling |
| Scrolling | Mouse wheel | Mouse wheel | Browser scroll input reaches the focused terminal | browser-adapted | browser manual: mouse scrolling |
| Selection | Character selection and yank | `v`, motion, `y` | Native selection; yank text goes through clipboard bridge | browser-adapted | `parity_core::selection_yank_and_quit_set_native_state`; browser manual: clipboard |
| Selection | Line selection | `V`, motion, `y` | Native line selection and clipboard bridge | browser-adapted | `parity_core::selection_yank_and_quit_set_native_state`; browser manual: clipboard |
| Confirm | Quit conversation | `q`, then `y` / `n` / `Esc` | Native confirmation; `y` routes to shell quit event | browser-adapted | browser manual: quit and reset |
| Confirm | Delete messages | `d`, then `y` / `n` / `Esc` | Native confirmation and local simulated delete | simulated | `parity_core::invalid_count_clears_and_visual_delete_asks_for_confirmation`; `parity_core::delete_uses_fixture_state` |
| Confirm | Discard draft | `:discard`, then confirm | Native confirmation | simulated | `parity_core::discard_command_confirms_and_clears_the_draft` |
| Drafts | Edit and switch chats | `i`, type, `Esc`, switch away and back | Native in-memory draft store | simulated | `parity_core::emoji_completion_and_draft_survive_chat_switch` |
| Drafts | Reset | Reset button | New demo world clears drafts and local changes | browser-adapted | browser manual: reset clears state |
| Emoji | Complete shortcode | Type `:smile`, `Tab` or `Enter` | Native popup inserts emoji into composer | simulated | `parity_core::emoji_completion_and_draft_survive_chat_switch` |
| Emoji | Filter and dismiss | Type shortcode, `Up` / `Down` / `Esc` | Native popup filtering and dismissal | simulated | `tui::app::tests::a_character_keeps_filtering_and_keeps_the_popup_open`; `tui::app::tests::escape_puts_the_completion_away_and_leaves_the_draft_alone`; browser manual: emoji popup |
| Rename | Set or clear contact name | `:rename` | Native local name action updates chat and profile | simulated | `parity_core::rename_sets_and_clears_local_name` |
| Messaging | Send | `Enter` | Native optimistic send and deterministic simulator result | simulated | `parity_core::send_and_reply_use_fixture_state`; `parity_core::send_failure_dismiss_and_attachment_retry_follow_native_actions` |
| Messaging | Edit, delete, reply | `e`, `d`, `r` | Native message operations with local simulator effects | simulated | `parity_core::edit_uses_fixture_state`; `parity_core::delete_uses_fixture_state`; `parity_core::send_and_reply_use_fixture_state` |
| Messaging | Failed send, retry upload, dismiss | `o`, `D` | Native failure state and upload actions | simulated | `parity_core::send_failure_dismiss_and_attachment_retry_follow_native_actions` |
| Quit | Immediate quit | `Ctrl-C` or `:q` | Real `App` sets quit state; driver emits `quit-demo` for shell release | browser-adapted | `parity_core::selection_yank_and_quit_set_native_state`; browser manual: quit and reset |
| Quit | Close browser tab | `Ctrl-W` | Native key still reaches `App`; prevent default while terminal has focus | browser-adapted | browser manual: Ctrl-W with terminal focused |
| Update | Install update | `:update` | “Updates are disabled in the offline demo.” | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| Chat list | Pin or unpin | `p` or profile action | Native `App` ordering plus simulator state | simulated | `parity_secondary::pins_archives_and_account_switches_update_the_fictional_world`; `simulator::local_mutations_persist_until_reset` |
| Chat list | Archive or restore | `:archive`, `:archived` | Native archive folder; opening archived chat and restoring uses native state | simulated | `parity_secondary::pins_archives_and_account_switches_update_the_fictional_world`; `tui::app::tests::archiving_moves_a_chat_and_unarchiving_puts_it_back` |
| Arrival | Blocked peer | `:block`, `:unblock` | Blocked peers produce no scheduled arrivals; unblocked peers resume arrivals | simulated | `simulator::blocked_peer_arrivals_are_suppressed_until_unblocked` |
| Contacts | List and resolve | `:contacts`, `:new <query>` | Seeded local contact candidates; empty contacts return empty results | simulated | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data` |
| Contacts | Profile and local name | `Enter`, `:rename [name]` | Native profile card and local alias state | simulated | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data`; `parity_core::rename_sets_and_clears_local_name` |
| Contacts | Report peer | `:report [reason]` | Report stays in simulator memory; no network report is sent | simulated | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data` |
| Account | Switch account | account card selection | Switches among three seeded accounts; local fixture data stays isolated | simulated | `parity_secondary::pins_archives_and_account_switches_update_the_fictional_world`; `fixtures::world_has_three_complete_fictional_accounts` |
| Account | Add account or logout | `:add-account`, `:logout` | “The demo has a fixed cast. Use the account card to switch.” | impossible | `simulator::unsupported_native_actions_say_why` |
| Session | Sign in and passphrase | `:signin`, `:unlock`, `:passphrase` | “Sign-in is disabled in the offline demo. Switch seeded accounts from the account card.”; “There is no session passphrase in the demo.” | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest`; `simulator::unsupported_native_actions_say_why` |
| Session | Import or export | `:import-session <path>`, `:export-session <path>` | “Session import is unavailable in the browser demo.”; “Session export is unavailable in the browser demo.” | impossible | `simulator::unsupported_native_actions_say_why` |
| Connection | Retry | `:retry` | No Telegram reconnect runs in the browser demo | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| History | Older and newer pages | scroll to either edge | Deterministic local pages apply through native `App` paging APIs | simulated | `parity_secondary::history_pages_and_read_receipts_apply_to_real_app_state` |
| Read state | Read receipt | open conversation | Scheduled receipt updates native message read state | simulated | `parity_secondary::history_pages_and_read_receipts_apply_to_real_app_state` |
| Search | Global filters and pages | `:search <query>` | Native search state receives filtered results in stable 20-result pages | simulated | `parity_secondary::filtered_global_search_pages_append_to_real_search_state`; `simulator::search_filters_and_global_pages_are_deterministic` |
| Media | Open and save | `Enter`, `:save` | Open uses the browser viewer; save starts an immediate fixture-byte download | browser-adapted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes`; browser manual: open and save media |
| Media | Cancel download | cancel media prompt | Browser downloads complete immediately; no in-flight download can be cancelled | browser-adapted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes` |
| Media | Send attachment | `:attach` | Browser file picker records the selected filename; file bytes are not read or uploaded | browser-adapted | `simulator::selected_attachment_records_filename_without_claiming_file_bytes`; browser manual: attach local file |
| Stickers | Render fixture sticker | conversation render | Real sticker decoder runs; token mode remains available when graphics are unavailable | browser-adapted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes`; browser manual: sticker fallback |
| Presence | Typing and online status | simulated time | Presence and typing updates reach real `App` state on deterministic timers | simulated | `simulator::scheduled_arrivals_respect_open_chat_and_timing` |
| Settings | Open settings | `:settings` | Native command opens the self profile card; browser shell keeps display settings | browser-adapted | `tui::app::tests::settings_command_opens_the_profile`; browser manual: settings card and shell controls |
| Simulator | Every TUI action | Native action dispatch | Every `Action` variant has an effect or an explicit refusal | simulated | `simulator::every_action_has_an_effect`; `simulator::unsupported_native_actions_say_why` |

## Action coverage

Every `tui::Action` variant has an explicit row below. The simulator test
`simulator::every_action_has_an_effect` checks all variants return a meaningful
effect; refusal rows also use `simulator::unsupported_native_actions_say_why`.

| Action variants | Demo outcome | Category | Evidence |
| :--- | :--- | :--- | :--- |
| `Update` | “Updates are disabled in the offline demo.” | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| `Send`, `Edit`, `Delete`, `Forward` | Mutate fictional messages through the local simulator | simulated | `parity_core::send_and_reply_use_fixture_state`; `simulator::edit_delete_and_forward_mutate_world` |
| `SendAttachment` | Browser file picker records a filename; bytes stay unread and unuploaded | browser-adapted | `simulator::selected_attachment_records_filename_without_claiming_file_bytes` |
| `CancelUpload`, `RetryUpload`, `DismissUpload` | Cancel, retry or dismiss a local simulated upload | simulated | `simulator::cancelled_upload_retries_once_without_early_world_message`; `parity_core::send_failure_dismiss_and_attachment_retry_follow_native_actions` |
| `FetchContact`, `FetchContacts`, `ResolveUser` | Read seeded profiles, contacts and candidates | simulated | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data` |
| `Search`, `GlobalSearch`, `MoreGlobal` | Search local messages and page deterministic results | simulated | `parity_secondary::filtered_global_search_pages_append_to_real_search_state` |
| `TogglePin`, `SwitchAccount` | Change the active fictional account's local state | simulated | `parity_secondary::pins_archives_and_account_switches_update_the_fictional_world` |
| `Login`, `LoginCancelled`, `AddAccount`, `Logout`, `ExportSession`, `ImportSession`, `SubmitSessionPassphrase`, `ChangeSessionPassphrase` | “Sign-in is disabled in the offline demo. Switch seeded accounts from the account card.”; “Sign-in cancelled; the demo stays signed in.”; “The demo has a fixed cast. Use the account card to switch.”; “Logout is disabled. Use Reset demo to start over.”; “Session export is unavailable in the browser demo.”; “Session import is unavailable in the browser demo.”; “There is no session passphrase in the demo.” | impossible | `simulator::unsupported_native_actions_say_why` |
| `OpenMedia`, `SaveMedia` | Open or download local fixture bytes through browser APIs | browser-adapted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes` |
| `CancelMediaDownload` | Report that browser fixture downloads complete immediately | browser-adapted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes` |
| `SetArchived`, `SetBlocked`, `SetLocalName`, `DeleteConversation`, `ReportPeer` | Apply local account and contact changes; no server request | simulated | `parity_secondary::pins_archives_and_account_switches_update_the_fictional_world`; `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data`; `simulator::local_mutations_persist_until_reset` |

## Unicode and bidi

| Flow | Demo behavior | Category | Evidence |
| :--- | :--- | :--- | :--- |
| Persian, Arabic, mixed direction and combining marks | Real `App` renders fixture text in Visual mode | simulated | `unicode::app_renders_unicode_corpus_without_losing_glyphs`; browser rendering runs in the gated `wasm` CI job |
| Bidi modes | Visual output is stable and differs from Terminal and Words output | simulated | `unicode::visual_render_is_stable_and_differs_from_terminal_order` |
| Composer and Backspace | Persian text and decomposed graphemes stay intact; Backspace removes one grapheme | simulated | `unicode::composer_keeps_rtl_text_and_deletes_one_grapheme` |
| Selection and yank | Character selection and yank preserve logical RTL order | simulated | `unicode::character_selection_yanks_original_logical_text` |
| RTL search prefix | Search matches Arabic and Persian prefixes without byte slicing | simulated | `unicode::arabic_search_uses_character_prefixes` |
| Emoji shortcode and following text | Full ZWJ emoji inserts without corrupting the next character | simulated | `unicode::emoji_shortcode_inserts_full_sequence_before_following_text` |
| Mobile IME | Composition sends committed text once; Enter and Backspace wait for composition to end | browser-adapted | Playwright synthetic composition test in `shell.spec.ts`; browser manual: mobile IME |

Browser text is one glyph per cell. Bidi reorder does not shape Arabic or join letters across cells. Browser screenshots do not prove terminal compatibility.

## Command outcomes

Every command in the product list has a row here. Supported commands use native `App` actions; unavailable commands preserve their simulator status sentence verbatim.

| Command | Demo outcome | Category | Evidence |
| :--- | :--- | :--- | :--- |
| `:q`, `:quit` | Native quit state; shell releases terminal | browser-adapted | `parity_core::command_quit_sets_native_quit_state`; browser manual: quit and reset |
| `:chat <id>` | Select seeded chat by id | simulated | `tui::app::tests::chat_command_selects_the_matching_chat` |
| `:new <query>` | Search seeded contacts and open selected result | simulated | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data`; browser manual: new conversation |
| `:search <query>` | Search across fixture messages with native filters and paging | simulated | `parity_secondary::filtered_global_search_pages_append_to_real_search_state` |
| `:more` | Append next fixture search page when more results exist | simulated | `parity_secondary::filtered_global_search_pages_append_to_real_search_state`; browser manual: global search |
| `:discard` | Confirm then clear current draft | simulated | `parity_core::discard_command_confirms_and_clears_the_draft` |
| `:attach` | Browser file picker records the selected filename; bytes stay unread and unuploaded | browser-adapted | `simulator::selected_attachment_records_filename_without_claiming_file_bytes`; browser manual: attach local file |
| `:open` | Open available fixture media in browser viewer | browser-adapted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes`; browser manual: open media |
| `:save <path>` | Browser download uses fixture bytes; path is not written | browser-adapted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes`; browser manual: save media |
| `:date <ISO>` | Jump to nearest fixture message date | simulated | `tui::app::tests::a_date_jump_label_names_the_day` |
| `:settings` | Open self profile card; shell display settings remain browser controls | browser-adapted | `tui::app::tests::settings_command_opens_the_profile`; browser manual: settings |
| `:update` | “Updates are disabled in the offline demo.” | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| `:signin` | “Sign-in is disabled in the offline demo. Switch seeded accounts from the account card.” | impossible | `simulator::unsupported_native_actions_say_why` |
| `:add-account` | “The demo has a fixed cast. Use the account card to switch.” | impossible | `simulator::unsupported_native_actions_say_why` |
| `:retry` | No Telegram reconnect runs in the browser demo | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| `:unlock` | “There is no session passphrase in the demo.” | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| `:passphrase` | “There is no session passphrase in the demo.” | impossible | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| `:export-session <path>` | “Session export is unavailable in the browser demo.” | impossible | `simulator::unsupported_native_actions_say_why` |
| `:import-session <path>` | “Session import is unavailable in the browser demo.” | impossible | `simulator::unsupported_native_actions_say_why` |
| `:contacts` | List seeded contacts; empty contact list stays empty | simulated | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data` |
| `:archive` | Move selected chat to archive | simulated | `parity_secondary::pins_archives_and_account_switches_update_the_fictional_world` |
| `:archived` | Open native archived-chat list | simulated | `parity_secondary::pins_archives_and_account_switches_update_the_fictional_world`; browser manual: archived list |
| `:block <peer>` | Block fixture peer and suppress later arrivals | simulated | `simulator::blocked_peer_arrivals_are_suppressed_until_unblocked` |
| `:unblock <peer>` | Unblock fixture peer and permit later arrivals | simulated | `simulator::blocked_peer_arrivals_are_suppressed_until_unblocked` |
| `:rename [name]` | Save or clear a local contact alias | simulated | `parity_core::rename_sets_and_clears_local_name` |
| `:delete-chat [both]` | Delete fixture chat locally; no server deletion occurs | simulated | `simulator::local_mutations_persist_until_reset`; browser manual: delete chat |
| `:report [reason]` | “Report recorded locally. No report was sent to Telegram.” | simulated | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data` |

## Overlay inventory

The `PromptKind` inventory is `Message`, `Command`, `Search`, `NewChat`,
`GlobalSearch`, `Reply`, `Edit`, `Phone`, `Code`, `Password`, `Passphrase`,
`NewPassphrase` and `RepeatPassphrase`. The `ConfirmKind` inventory is `Quit`,
`Restart`, `DiscardDraft`, `Logout`, `ImportSession`, `ChangePassphrase`,
`Attach`, `DeleteMessages`, `Block`, `DeleteConversation` and `Report`.

| Overlay | Native flow | Demo outcome | Evidence |
| :--- | :--- | :--- | :--- |
| Message, Command, Search | Compose, command, current-chat search | Native prompt; fixture actions | `parity_core::emoji_completion_and_draft_survive_chat_switch`; browser manual: prompt submit and cancel |
| NewChat, GlobalSearch | Contact discovery, global search | Native prompts; seeded contacts and local search pages | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data`; `parity_secondary::filtered_global_search_pages_append_to_real_search_state` |
| Reply, Edit | Compose reply or edit outgoing message | Native prompts; simulator updates fixture state | `parity_core::send_and_reply_use_fixture_state`; `parity_core::edit_uses_fixture_state` |
| Phone, Code, Password | Sign-in fields | Refused with “Sign-in is disabled in the offline demo. Switch seeded accounts from the account card.” | `simulator::unsupported_native_actions_say_why`; browser manual: sign-in refusal |
| Passphrase, NewPassphrase, RepeatPassphrase | Unlock or change session passphrase | Refused with “There is no session passphrase in the demo.” | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| Quit | Confirm browser demo exit | Shell handles exit event; Reset re-enters demo | `parity_core::selection_yank_and_quit_set_native_state`; browser manual: quit and reset |
| Restart | Confirm after update install | No update can be installed; status says “No update is installed in the offline demo.” | `simulator::unsupported_native_actions_say_why` |
| DiscardDraft | Confirm draft discard | Native confirmation clears local draft | `parity_core::discard_command_confirms_and_clears_the_draft` |
| Logout, ImportSession, ChangePassphrase | Confirm session mutation | Session actions remain unavailable; no account or file is changed | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| Attach | Confirm local media send | Browser file picker records the filename; no bytes are read or uploaded | `simulator::selected_attachment_records_filename_without_claiming_file_bytes`; browser manual: attach local file |
| DeleteMessages | Confirm selected message deletion | Deletes fixture messages locally | `parity_core::delete_uses_fixture_state` |
| Block | Confirm block or unblock | Updates local blocked set; no server request | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data` |
| Forward | Choose destination chat | Copies fixture messages with new local ids | `simulator::edit_delete_and_forward_mutate_world`; browser manual: forward message |
| Report | Confirm peer report | Records locally; says no Telegram report was sent | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data` |
| Update offer | Apply, restart, cancel, or later | Applying is unavailable; no update is downloaded or installed | `parity_secondary::browser_only_actions_keep_offline_claims_honest` |
| Contacts overlay | Browse, select, cancel | Native directory over seeded contacts; empty result is preserved | `parity_secondary::contacts_profiles_blocks_reports_and_empty_results_use_seeded_data`; browser manual: contacts |
| Media dialog | Open, save, or close | Browser viewer/download handles fixture bytes; save completes immediately and no local path is persisted | `parity_secondary::media_open_save_cancel_and_token_sticker_use_fixture_bytes`; browser manual: media and stickers |
| Clipboard fallback | Deny browser clipboard permission | Demo selects copied text and shows the manual-copy instruction | `shell.spec.ts` clipboard denial fallback in gated `wasm` CI |

## Source inventory

| Source | Inventory result |
| :--- | :--- |
| `README.md` essential controls and command list | Core controls and every command appear above or in Command outcomes. |
| `docs/product.md` feature set | Messaging, selections, emoji, aliases, updates, profiles, media, presence, retries, search, and account management appear above. |
| `crates/tui/src/state/coordinate.rs` | Mode, focus, prompt, confirm, motion, selection, and operator dispatch are represented above. Code is authoritative where README wording differs. |
| `crates/tui/src/line.rs` | Composer editing, shortcode completion, and input keys are represented above. |
| `crates/tui/src/card.rs` | Profile card navigation, rename, block, report, and account actions are represented above. |
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
- **quit and reset:** Use `:q` and `Ctrl-C`; shell handles `quit-demo`, then Reset re-enters.
- **Ctrl-W:** Focus terminal, press `Ctrl-W`, verify tab remains and native line behavior occurs.
- **update refusal:** Open update flow and verify “Updates are disabled in the offline demo.”
- **secondary chat list:** Pin/unpin, archive/unarchive, open archived list, and verify selected chat remains selected.
- **contacts and profile:** Open contacts, search a new conversation, rename, block/unblock, report, and switch seeded accounts.
- **history and receipts:** Scroll older/newer, jump with `:date`, and verify local read state updates.
- **global search:** Search text plus `media:`, `since:`, and `until:` filters; page results and open a hit.
- **media and stickers:** Attach local file, open/save fixture media, and verify sticker fallback.
- **session refusal:** Try sign-in, unlock, passphrase, import/export, add-account, and logout; verify each exact refusal sentence.
- **connection and settings:** Confirm the demo stays offline; open `:settings` and use browser display controls.
