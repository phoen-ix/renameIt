# E2E coverage

Every user-facing feature of the GUI and every command and flag of `ren-cli`,
each mapped to the test that drives it end to end. Run them all with
`scripts/e2e.sh`. That script runs the GUI suites (`crates/ren-gui/tests/gui.rs`
and `e2e.rs`) and the CLI suites (`crates/ren-cli/tests/*.rs`).

The GUI tests drive the real `RenameItApp` headlessly through its
accessibility tree (egui_kittest, D24). The CLI tests run the built binary as a
process. The screenshot suite `snapshots.rs` (D248) is the docs tour, not part
of this map.

**Inventory**, 2026-11-01 (P115 rows added the same day). Re-run these commands from the repo root to check
the list below:

```sh
ls crates/ren-gui/src/panels/*.rs crates/ren-gui/src/editors/*.rs | grep -vE '/(mod|assist)\.rs'   # 32 zones, windows and editors
grep -oE 'label: "[^"]+"' crates/ren-gui/src/panels/catalogue.rs                                   # 18 operations
grep -A15 'pub const ALL' crates/ren-gui/src/panels/settings.rs | grep -c 'Self::'                # 14 Settings pages
grep -oE 'Key::[A-Za-z0-9]+' crates/ren-gui/src/app.rs | sort -u                                   # 18 keys
grep -A17 'pub enum RowAction' crates/ren-gui/src/panels/rows.rs                                   # row menu
awk '/^enum Command|^enum PresetAction|^struct JobArgs|^struct Cli/,/^}/' crates/ren-cli/src/main.rs \
  | grep -oE '^    [A-Z][a-z]+|^ +[a-z_]+:'                                                         # CLI commands and flags
grep -oE "'[a-z?]' =>" crates/ren-cli/src/compat.rs                                                # legacy switches
```

The rows also cover the dialogs, banners and error states those modules draw.

**Totals:** 259 numbered rows plus the 3 sweep rows. 254 rows are ✅ and 5 are ❌,
all (c) covered at another layer.

Roles: none. This is a single-user desktop app and a CLI, with no accounts.

`gui ›` means `crates/ren-gui/tests/gui.rs`, `e2e ›` means
`crates/ren-gui/tests/e2e.rs`, and `cli/<file> ›` means
`crates/ren-cli/tests/<file>.rs`.

## Sweep (every screen once)

| Screen kind | Test |
|---|---|
| All 18 operation editors, added by button and by Ctrl+K and picked from the card's drop-down | ✅ e2e › sweep_every_operation_in_the_drop_down_opens_its_editor |
| All 14 Settings pages, from the rail | ✅ e2e › sweep_every_settings_page_is_reachable_from_the_rail |
| Settings ▸ About, presets drawer, Include filter, row menu, Grid/List | ✅ e2e › sweep_every_window_and_popover_opens_and_closes |

## Source bar (`panels/source_bar.rs`)

| # | Feature | Test |
|---|---|---|
| 1 | Browser mode lists the folder | ✅ gui › the_window_lists_the_folder_it_was_pointed_at |
| 2 | Browser ⇄ Free Select switch | ✅ e2e › the_mode_switch_and_free_selects_add_and_clear |
| 3 | Address box: type a path, Enter | ✅ e2e › a_path_typed_into_the_address_box_is_listed_on_enter |
| 4 | 📂 folder picker button | ✅ e2e › the_folder_button_lists_the_folder_it_returns |
| 5 | Pattern box | ✅ gui › the_pattern_box_is_named_by_its_label |
| 6 | ⟳ Refresh button | ✅ gui › the_refresh_button_reads_the_files_again_as_f9_does |
| 7 | Include chips Files / Folders / Subfolders | ✅ e2e › the_include_chips_decide_what_is_listed |
| 8 | Include filter popover (include, typed) | ✅ gui › the_include_filter_popover_takes_a_click_and_then_typing |
| 9 | Include filter: what it narrows | ✅ gui › the_include_filter_narrows_which_files_change |
| 10 | List ⇄ Grid switch | ✅ e2e › sweep_every_window_and_popover_opens_and_closes |
| 11 | ⚙ ▸ Settings | ✅ e2e › the_gear_opens_settings_and_about_is_its_last_page, sweep_every_settings_page_is_reachable_from_the_rail |
| 12 | Settings ▸ About, the last page | ✅ gui › about_opens_and_reports_what_the_session_did |
| 13 | Free Select: Add files… | ✅ e2e › the_mode_switch_and_free_selects_add_and_clear |
| 14 | Free Select: Clear | ✅ e2e › the_mode_switch_and_free_selects_add_and_clear |
| 15 | Free Select: drag and drop | ✅ gui › dropped_files_populate_free_select |
| 16 | Error state: folder that is gone | ✅ gui › a_path_that_is_gone_does_not_stop_the_app_starting |
| 17 | Error state: "⚠ N item(s) could not be read" | ✅ e2e › an_unreadable_subfolder_is_counted_beside_the_path (Unix) |
| 18 | Command-line-limit banner: List the whole folder | ✅ gui › a_selection_near_the_command_line_limit_offers_the_whole_folder |
| 19 | Command-line-limit banner: Dismiss | ✅ e2e › the_selection_limit_banner_can_be_dismissed |
| 20 | No banner for a drag | ✅ gui › a_drag_of_many_files_is_told_nothing_about_a_limit |

## Pipeline: the card stack (`panels/operation.rs`, `panels/catalogue.rs`)

| # | Feature | Test |
|---|---|---|
| 21 | + Add operation adds a default card, expanded | ✅ gui › add_operation_adds_a_default_card_and_opens_it |
| 22 | Ctrl+K adds a default card | ✅ gui › ctrl_k_adds_a_default_card |
| 23 | Card type drop-down offers every operation and switches the card | ✅ gui › a_cards_drop_down_offers_every_operation_and_switches_to_the_one_you_pick |
| 24 | A switched card is seeded like a new one, keeps its switch | ✅ gui › a_switched_card_is_seeded_like_a_new_one_and_keeps_its_switch |
| 25 | Settings ▸ Default operation | ✅ gui › the_default_operation_is_set_in_settings_and_remembered |
| 26 | An empty pipeline starts with one default card | ✅ `crates/ren-gui/src/app.rs` unit test › an_empty_pipeline_starts_with_one_default_card |
| 27 | New card lands after the open one, expanded | ✅ gui › a_new_card_is_added_after_the_open_one_and_expanded |
| 28 | Card header checkbox (on/off) | ✅ e2e › the_card_header_switches_a_card_off_and_its_menu_edits_the_stack |
| 29 | Disabled card leaves the preview | ✅ gui › disabling_a_card_takes_it_out_of_the_preview |
| 30 | Card summary click collapses / expands | ✅ gui › collapsing_the_card_closes_the_strip |
| 31 | Drag handle reorders | ✅ gui › dragging_a_card_by_its_handle_reorders_the_stack |
| 32 | ⋮ Move up | ✅ e2e › the_card_header_switches_a_card_off_and_its_menu_edits_the_stack |
| 33 | ⋮ Move down | ✅ gui › the_overflow_menu_reorders_the_stack |
| 34 | ⋮ Duplicate | ✅ e2e › the_card_header_switches_a_card_off_and_its_menu_edits_the_stack |
| 35 | Duplicates are independent | ✅ gui › duplicating_a_card_gives_two_independently_editable_cards |
| 36 | ⋮ Delete | ✅ e2e › the_card_header_switches_a_card_off_and_its_menu_edits_the_stack |
| 37 | Error state: no card left blocks the run | ✅ gui › deleting_the_last_card_blocks_the_run_with_a_reason |
| 38 | Order changes the result | ✅ gui › reordering_two_cards_changes_the_result |
| 39 | Three cards compose, run and undo as one | ✅ gui › a_three_card_pipeline_previews_composes_and_undoes_as_one |
| 40 | Card scope expander | ✅ gui › an_action_card_scope_offers_only_the_filter |
| 41 | Per-card filter | ✅ gui › a_per_card_filter_narrows_only_that_card |
| 42 | Source-bar filter as the default | ✅ gui › the_source_bar_filter_is_the_default_for_a_card_that_has_none |
| 43 | Pre-processor: enable from the card | ✅ gui › enabling_a_pre_processor_from_the_card_starts_it_as_the_identity |
| 44 | Pre-processor: stages keep what was typed | ✅ gui › unticking_a_pre_processor_stage_and_ticking_it_again_brings_back_what_was_typed |
| 45 | Pre-processor: regex switch | ✅ gui › the_pre_processor_regex_switch_unticks_and_new_searches_follow_it |
| 46 | Pre-processor from a file, cleared | ✅ gui › a_pre_processor_that_came_from_a_file_is_shown_and_can_be_cleared |
| 47 | Card stack survives a restart | ✅ gui › the_card_stack_survives_a_restart |
| 48 | Typing over 5 000 files keeps up (8 cards) | ✅ gui › an_eight_card_pipeline_keeps_up_with_typing_over_five_thousand_files |

## Operations (`editors/*.rs`)

| # | Operation / control | Test |
|---|---|---|
| 49 | Replace: Find / Replace with typed | ✅ e2e › the_run_controls_simulate_rename_fold_the_log_and_undo |
| 50 | Replace: regex error clears when finished | ✅ gui › a_regex_that_is_finished_being_typed_clears_its_error |
| 51 | Replace: add to the Batch Replace list | ✅ gui › add_to_batch_replace_carries_all_seven_settings_not_just_the_two_boxes |
| 52 | Batch Replace card keeps its own rules | ✅ gui › a_batch_replace_card_keeps_its_own_rules |
| 53 | Batch Replace: delete a rule | ✅ gui › deleting_a_batch_rule_takes_its_broken_tag_with_it |
| 54 | Batch Replace rule ⋯ popover: case, swap, skip, max | ✅ gui › a_batch_rules_options_stay_open_while_they_are_used, a_batch_rules_skip_and_max_are_named |
| 55 | Set Casing: mode radios | ✅ e2e › the_casing_editor_applies_the_mode_clicked |
| 56 | Set Casing: its own exception words | ✅ gui › a_set_casing_card_edits_its_own_exception_words |
| 57 | Add / Remove: Insert, at pos, Overwrite | ✅ e2e › the_add_remove_editor_inserts_at_the_position_typed |
| 58 | Move Section (through its ⌖ markers) | ✅ gui › the_section_marker_sets_both_the_position_and_the_length |
| 59 | Space Trimming switches | ✅ e2e › the_space_trimming_switches_reach_the_preview |
| 60 | General operations preview correctly | ✅ gui › every_general_operation_previews_what_it_should |
| 61 | Add Counter: placement, separator | ✅ e2e › the_add_counter_editor_moves_the_counter_and_its_separator |
| 62 | Re-Number, Zero Padding, Add Counter results | ✅ gui › every_numbers_operation_previews_what_it_should |
| 63 | Zero Padding: Digits | ✅ e2e › the_zero_padding_editor_pads_to_the_digits_given |
| 64 | Music Rename: style typed, files renamed from tags | ✅ gui › music_rename_names_files_from_their_tags |
| 65 | Music Rename: untagged file left alone | ✅ gui › music_rename_leaves_an_untagged_file_alone |
| 66 | Music Rename: (edit styles) list | ✅ gui › the_music_styles_list_is_editable_and_the_card_keeps_its_pattern |
| 67 | Music Tagger: refused until confirmed | ✅ gui › the_tag_operations_are_offered_but_refused_until_they_are_confirmed |
| 68 | Music Tagger: ✨ Quick Setup | ✅ gui › quick_setup_configures_the_parts_pattern_and_the_fields_together |
| 69 | Music Tagger: genre from Setup Parts | ✅ gui › a_genre_from_setup_parts_can_be_kept |
| 70 | Remove Tags | ✅ gui › remove_tags_starts_with_nothing_selected |
| 71 | Free Format: Pattern | ✅ e2e › the_free_format_editor_builds_the_name_from_its_pattern |
| 72 | Set Attributes: four tri-state boxes | ✅ gui › each_attribute_box_writes_through_to_its_own_bit |
| 73 | Set Attributes: unsupported ones greyed | ✅ gui › the_attributes_this_platform_cannot_write_are_shown_greyed_with_a_reason |
| 74 | Set Date & Time: starts at today | ✅ gui › a_new_set_date_card_starts_at_today |
| 75 | Set Date & Time: date and time typed | ✅ gui › a_date_typed_one_key_at_a_time_is_the_date_that_was_typed |
| 76 | CSV List Rename: choose a file | ✅ e2e › the_csv_editor_renames_from_the_list_it_was_given |
| 77 | Filename Editor: lines typed | ✅ gui › three_lines_typed_into_a_filename_editor_are_three_lines |
| 78 | Filename Editor: copy the current list | ✅ gui › a_filename_editor_card_counts_the_lines_it_was_just_given |
| 79 | Scripting: a write is confirmed | ✅ gui › a_script_write_is_confirmed_before_it_happens |
| 80 | Error state: Scripting with a missing script | ✅ gui › a_missing_script_is_not_said_to_take_no_arguments |
| 81 | `<tags>` menu | ✅ gui › the_tag_menu_lists_every_music_tag_the_engine_has |
| 82 | Error state: a mistyped tag | ✅ gui › a_mistyped_tag_is_reported_in_the_editor |
| 83 | Field history | ✅ gui › the_history_holds_no_duplicates_and_stops_at_twelve |

## Visual Assist (`panels/visual_assist.rs`)

| # | Feature | Test |
|---|---|---|
| 84 | ⌖ opens the strip on its own card | ✅ gui › the_marker_button_opens_the_strip_on_its_own_card |
| 85 | Select lifts the text | ✅ gui › select_lifts_the_chosen_text_into_the_find_box |
| 86 | Cancel writes nothing | ✅ gui › cancel_writes_nothing |
| 87 | Select refused with nothing selected | ✅ gui › select_refuses_until_something_is_selected |
| 88 | Error state: card handed nothing | ✅ gui › a_card_that_is_handed_nothing_says_why |
| 89 | F3 opens it, even from a field | ✅ gui › f3_opens_visual_assist_even_while_a_field_has_focus |
| 90 | F3 cycles a Both card | ✅ gui › f3_cycles_the_two_targets_of_a_both_card |
| 91 | F3 on a card without it | ✅ gui › f3_says_so_when_the_card_has_no_visual_assist |
| 92 | The strip across sort, run, undo, duplicate, move, disable | ✅ gui › the_strip_survives_a_re_sort, a_run_closes_the_strip_and_a_simulation_does_not, an_undo_closes_the_strip, duplicating_the_card_closes_the_strip, moving_the_card_keeps_the_strip_and_recomputes_it, disabling_the_card_keeps_the_strip_and_explains |

## Run-wide settings, drawn in the cards that use them (`panels/run_settings.rs`, D254)

| # | Feature | Test |
|---|---|---|
| 93 | Counter setup: Start, Step, Reset at | ✅ e2e › the_add_counter_cards_counter_boxes_drive_the_counter; gui › the_reset_at_number_is_named_by_its_checkbox |
| 94 | Counter setup: padding | ✅ gui › the_counter_setup_panel_drives_the_counter |
| 95 | Counter: Reset each folder / on base name change | ✅ gui › the_counters_boxes_tick_in_the_add_counter_card (what they do: `crates/ren-core/src/counter.rs` unit tests) |
| 96 | Running counter carries over | ✅ gui › a_running_counter_picks_up_where_the_last_batch_left_off |
| 97 | Setup parts pattern feeds `<%n>` | ✅ gui › the_parts_pattern_feeds_the_part_tags |
| 98 | Setup parts ✨ detect | ✅ gui › the_parts_wand_fills_the_pattern_in_the_tagger_card |
| 99 | Each section shows only in the cards that use it | ✅ gui › each_run_wide_setting_shows_in_the_cards_that_use_it |
| 100 | Only rename if all tags are available | ✅ e2e › only_if_all_tags_are_available_leaves_an_untagged_file_alone |

## File list and grid (`panels/file_table.rs`, `rows.rs`, `grid.rs`, `tile.rs`)

| # | Feature | Test |
|---|---|---|
| 101 | Columns fill the pane | ✅ gui › the_columns_fill_the_pane_rather_than_hugging_their_contents |
| 102 | Sort by header, the arrow on its header | ✅ gui › the_new_name_header_keeps_its_arrow_to_itself |
| 103 | Click to select scopes the run | ✅ gui › selecting_rows_scopes_the_run |
| 104 | Full-row select | ✅ gui › full_row_select_makes_the_whole_row_a_click_target |
| 105 | Ctrl+A | ✅ gui › ctrl_a_selects_every_row_the_list_is_showing |
| 106 | Arrow keys | ✅ gui › the_arrow_keys_walk_the_file_list |
| 107 | Home / End | ✅ e2e › home_and_end_jump_to_the_ends_of_the_list |
| 108 | Enter into a folder | ✅ gui › enter_walks_into_the_folder_under_the_keyboard |
| 109 | Backspace up a level | ✅ gui › backspace_goes_up_one_level_and_lands_on_the_folder_it_left |
| 109a | `..` above the list and the grid, double-clicked | ✅ e2e › a_double_click_on_dot_dot_goes_up_in_the_list_and_the_grid |
| 109b | No `..` in Free Select | ✅ e2e › free_select_has_no_dot_dot |
| 110 | Enter on a file does nothing | ✅ gui › enter_on_a_file_does_not_go_anywhere |
| 111 | Delete removes Free Select rows only | ✅ gui › delete_removes_rows_from_free_select_and_leaves_the_files |
| 112 | F2 inline rename, and undo | ✅ gui › f2_renames_a_single_row_and_undo_covers_it |
| 113 | F2 selects the stem | ✅ gui › f2_selects_the_name_so_typing_replaces_it_and_keeps_the_extension |
| 114 | F2 case-only rename | ✅ gui › f2_accepts_a_case_only_rename |
| 115 | Error state: inline rename onto an existing name | ✅ gui › a_manual_rename_onto_an_existing_name_is_refused |
| 116 | Enter jumps to the next file | ✅ gui › the_jump_follows_the_file_the_sort_puts_next_and_not_the_row_number |
| 117 | Row drag reorders the counter | ✅ gui › dragging_a_file_up_the_list_changes_the_number_the_counter_gives_it |
| 118 | Row menu: right-click selects | ✅ gui › right_clicking_an_unselected_row_selects_it |
| 119 | Row menu: Rename… | ✅ e2e › the_row_menus_rename_opens_the_inline_editor |
| 120 | Row menu: Show in file manager | ✅ e2e › show_in_file_manager_reveals_the_row_and_the_problem_solver_its_folders |
| 121 | Row menu: Copy to clipboard (names, new names) | ✅ gui › the_menu_and_the_row_menu_copy_the_same_list |
| 122 | Copy: one row per line | ✅ gui › copying_previews_produces_one_row_per_line |
| 123 | Row menu: Add to Free Select | ✅ gui › the_row_menu_adds_files_to_free_select |
| 124 | Row menu: Remove from Free Select | ✅ gui › the_row_menu_removes_rows_from_free_select |
| 125 | Empty folder, both views | ✅ gui › an_empty_folder_says_how_to_fill_it_in_both_views |
| 126 | Empty Free Select, both views | ✅ gui › an_empty_free_select_says_how_to_fill_it_in_both_views |
| 127 | Thumbnails in the list | ✅ gui › a_picture_gets_a_thumbnail_in_the_list |
| 128 | Grid shows the pictures | ✅ gui › the_grid_shows_the_pictures_the_list_would |
| 129 | Grid tile shows the new name and what the run does | ✅ gui › a_tile_shows_the_name_the_run_will_write |
| 130 | Grid with every tile filtered out | ✅ gui › a_grid_with_every_tile_filtered_out_says_so |
| 131 | Grid tile that is not a picture takes a click | ✅ gui › a_tile_that_is_not_a_picture_still_takes_a_click |
| 132 | Name that is not text | ✅ gui › a_name_that_is_not_text_says_so_and_can_still_be_renamed |
| 133 | Error state: a conflict blocks Rename | ✅ gui › a_conflict_blocks_the_rename_button |

## Run controls (`panels/run_controls.rs`: the bar above the table, the footer under the pipeline)

| # | Feature | Test |
|---|---|---|
| 134 | Rename button | ✅ e2e › the_run_controls_simulate_rename_fold_the_log_and_undo |
| 135 | Simulate checkbox + Simulate N files | ✅ e2e › the_run_controls_simulate_rename_fold_the_log_and_undo |
| 136 | Undo button | ✅ e2e › the_run_controls_simulate_rename_fold_the_log_and_undo |
| 137 | Log button, and the log after a run | ✅ e2e › the_run_controls_simulate_rename_fold_the_log_and_undo |
| 138 | Log at full height | ✅ gui › the_run_log_opens_at_its_full_height |
| 139 | Log names a created folder | ✅ gui › the_log_names_a_folder_the_run_created |
| 140 | Row filter chips All / Changed / Conflicts | ✅ e2e › the_row_filter_chips_narrow_the_rows |
| 141 | Action-only run reported | ✅ gui › an_action_only_run_reports_what_it_did |
| 142 | Simulated action-only run | ✅ gui › a_simulated_action_only_run_says_what_it_would_have_done |
| 143 | Undo of a mixed batch says what stayed | ✅ gui › undoing_a_mixed_batch_says_what_could_not_be_taken_back |
| 144 | Tag-only run does not arm Undo | ✅ gui › a_run_that_only_wrote_tags_does_not_arm_undo |
| 145 | Error state: system folder blocks the run | ✅ gui › a_guarded_folder_blocks_the_run_with_a_reason |
| 146 | Cancel a running batch | ✅ e2e › cancel_stops_a_running_batch_after_the_file_under_way (Undo after a cancel: `crates/ren-gui/src/app.rs` › a_cancelled_run_stops_after_the_op_under_way_and_is_undoable) |
| 147 | "preview failed" (engine panic) | ❌ (c) `crates/ren-gui/src/viewmodel/preview.rs` › a_panic_in_the_engine_is_reported_and_the_worker_survives; no input makes the engine panic, so the GUI cannot be driven to it |
| 148 | Batch undone in another window | ✅ gui › a_batch_undone_elsewhere_leaves_the_undo_stack |
| 149 | Free Select run keeps its rows | ✅ gui › a_free_select_run_keeps_its_rows |
| 150 | Free Select file gone costs one row | ✅ gui › a_free_select_file_that_is_gone_costs_only_its_own_row |

## Dialogs and banners (`panels/confirm.rs`, `ask.rs`, `about.rs`)

| # | Feature | Test |
|---|---|---|
| 151 | Irreversible-change confirmation | ✅ gui › the_tag_operations_are_offered_but_refused_until_they_are_confirmed |
| 152 | Consent lapses when the pipeline changes | ✅ gui › consent_does_not_survive_a_change_to_the_pipeline |
| 153 | One dialog only | ✅ gui › asking_again_while_the_dialog_is_up_does_not_stack_dialogs |
| 154 | `<Ask>` modal | ✅ gui › the_ask_modal_is_typed_into_straight_away_and_enter_renames |
| 155 | `<Ask>` answers forgotten when no longer asked | ✅ gui › an_ask_answer_is_forgotten_when_the_pipeline_stops_asking |
| 156 | About | ✅ gui › about_opens_and_reports_what_the_session_did |
| 157 | Recovery banner: Roll back | ✅ gui › roll_back_runs_off_the_frame |
| 158 | Recovery banner: Leave as is | ✅ e2e › the_recovery_banner_can_be_left_as_it_is |
| 159 | Recovery banner names a half-written file | ✅ gui › the_recovery_banner_names_the_file_that_was_being_written |
| 160 | Error state: unreadable journal | ✅ gui › an_unreadable_journal_is_named_in_the_banner |
| 161 | Error state: journal of a run in another window | ✅ gui › a_run_in_another_window_is_named_and_not_offered_for_rollback |

## Presets drawer (`panels/presets.rs`)

| # | Feature | Test |
|---|---|---|
| 162 | Open (Presets ▾) / close (✖) | ✅ e2e › sweep_every_window_and_popover_opens_and_closes |
| 163 | Save as preset | ✅ e2e › the_presets_drawer_saves_renames_duplicates_exports_deletes_and_imports |
| 164 | Load | ✅ gui › a_preset_round_trips_through_the_drawer |
| 165 | Append | ✅ e2e › the_presets_drawer_appends_and_runs |
| 166 | Run | ✅ e2e › the_presets_drawer_appends_and_runs |
| 167 | Rename… / Duplicate / Export… / Delete (Keep, Delete) / Import… | ✅ e2e › the_presets_drawer_saves_renames_duplicates_exports_deletes_and_imports |
| 168 | Import… is on screen; the rename box has a name | ✅ gui › the_drawers_import_button_is_on_screen_and_imports, the_preset_rename_box_has_a_name |
| 169 | Name box can be cleared | ✅ gui › the_preset_name_box_can_be_cleared |
| 170 | Presets survive a restart | ✅ gui › a_preset_survives_a_restart |
| 171 | Error state: unreadable preset listed | ✅ gui › an_unreadable_preset_is_listed_with_its_error |

## Settings (`panels/settings.rs`, 10 pages)

| # | Feature | Test |
|---|---|---|
| 172 | Batch Replace page: edit the default list | ✅ gui › the_settings_window_edits_the_list_a_new_batch_replace_card_starts_from |
| 173 | Batch Replace page: add a blank rule | ✅ gui › the_batch_replace_list_adds_a_blank_rule |
| 174 | Music styles page | ✅ gui › the_music_styles_list_is_editable_and_the_card_keeps_its_pattern |
| 175 | Casing words page | ✅ gui › the_casing_exception_list_is_a_default_for_new_cards |
| 176 | Columns & rows: columns on, Name always on | ✅ gui › the_display_page_turns_a_column_on_and_the_table_shows_it |
| 177 | Columns & rows: Restore defaults; Music styles: Restore defaults | ✅ e2e › the_display_page_stores_its_switches; gui › a_restored_string_list_row_does_not_inherit_the_old_rows_undo |
| 178 | Columns & rows: stripes; Thumbnails: border | ✅ e2e › the_display_page_stores_its_switches |
| 179 | Which files show: hidden, write-protected, pattern scope | ✅ e2e › the_file_system_page_changes_what_is_listed |
| 180 | Which files show: System files switch | ✅ e2e › the_file_system_page_changes_what_is_listed (the switch; what it hides is a Windows attribute, so Linux lists nothing differently: `crates/ren-platform/src/visibility.rs` › nothing_is_a_system_file_here) |
| 181 | Which files show: system-folder guard | ✅ gui › a_guarded_folder_blocks_the_run_with_a_reason |
| 182 | When RenameIt starts page | ✅ gui › the_startup_page_stores_a_switch_that_a_later_start_obeys |
| 183 | Right-click menu page (was Shell Integration) | ✅ gui › the_shell_page_says_what_this_platform_can_do |
| 184 | Right-click menu: install / refresh the menu (Windows) | ❌ (c) `crates/ren-gui/src/app.rs` MenuSpy tests and `panels/settings.rs` › LockedMenu; Linux has no menu to install |
| 185 | Appearance: theme | ✅ gui › the_theme_control_lives_in_settings_now |
| 186 | Appearance: interface size | ✅ gui › the_interface_size_scales_the_window_and_the_reset_puts_it_back |
| 187 | Where files are kept: folders | ✅ gui › the_problem_solver_page_names_the_folders_the_app_really_uses |
| 188 | Where files are kept: 📂 opens a folder | ✅ e2e › show_in_file_manager_reveals_the_row_and_the_problem_solver_its_folders |
| 189 | Where files are kept: Reset all settings… (confirm) | ✅ gui › the_reset_button_asks_first_then_puts_every_setting_back |
| 190 | Reset declined | ✅ gui › declining_the_reset_changes_nothing |
| 191 | Window keeps its size across pages | ✅ gui › the_settings_window_does_not_change_size_when_you_change_page |

## Hotkeys (`app.rs`)

| # | Key | Test |
|---|---|---|
| 192 | F4 undo / Ctrl+Z | ✅ gui › f4_undoes_the_last_batch |
| 193 | F5 run, F4 undo with a number box focused | ✅ gui › the_hotkeys_stay_live_while_a_number_box_has_focus |
| 194 | F6 address box | ✅ gui › f6_focuses_the_address_box |
| 195 | F8 Settings | ✅ gui › f8_opens_the_settings_window |
| 196 | F9 refresh | ✅ gui › refreshing_reads_the_files_again_rather_than_trusting_what_it_remembers |
| 197 | F12 folder picker | ✅ gui › f12_opens_the_folder_browser_and_lists_what_it_returns |
| 198 | Typing, modals and Ctrl+Shift+Z stand the keys down | ✅ gui › undo_and_run_hotkeys_stand_down_while_the_user_is_typing, the_hotkeys_stand_down_behind_a_modal, ctrl_shift_z_is_not_undo |

F2, F3, Ctrl+K, Ctrl+A, the arrows, Home/End, Enter, Backspace and Delete are
the rows in the sections above. F1 is deliberately unbound (P64).

## Launch (`launch.rs`)

| # | Feature | Test |
|---|---|---|
| 199 | A folder on the command line | ✅ gui › a_folder_on_the_command_line_is_where_the_app_opens |
| 200 | A relative folder | ✅ gui › a_relative_folder_on_the_command_line_is_made_absolute |
| 201 | A file: Start in its folder | ✅ gui › start_in_browses_the_folder_a_file_lives_in |
| 202 | A preset from the shell menu previews | ✅ gui › a_preset_from_the_menu_previews_rather_than_renaming |
| 203 | Error state: preset gone / command line unreadable | ✅ gui › a_preset_that_is_gone_still_opens_the_app_and_says_so, a_command_line_we_could_not_read_still_opens_and_explains |

## ren-cli (`crates/ren-cli/src/main.rs`, `compat.rs`, `exit.rs`)

| # | Command / flag | Test |
|---|---|---|
| 204 | `preview`, `apply`, `undo` round trip | ✅ cli/roundtrip › apply_then_undo_round_trips_byte_exactly |
| 205 | `preview` writes nothing | ✅ cli/roundtrip › preview_changes_nothing |
| 206 | `apply --simulate` | ✅ cli/roundtrip › simulate_reports_the_plan_without_touching_the_disk |
| 207 | `apply --allow-irreversible` | ✅ cli/preset › a_job_file_that_writes_tags_fails_closed_until_the_flag_is_given |
| 208 | `undo` with nothing to undo | ✅ cli/roundtrip › undo_with_no_transactions_fails_cleanly |
| 209 | `undo --txn` | ✅ cli/roundtrip › a_journal_in_use_is_refused_by_undo_and_left_alone_by_recover |
| 210 | `undo` names what it could not take back | ✅ cli/preset › undo_says_which_changes_it_could_not_take_back |
| 211 | `recover`, `recover --rollback` | ✅ cli/flags › recover_reports_an_unfinished_run_and_rollback_puts_it_back |
| 212 | `presets list` / `import` | ✅ cli/preset › a_preset_survives_a_restart_and_runs_by_name |
| 213 | `presets show` / `export` | ✅ cli/preset › export_writes_a_file_that_import_reads_back |
| 214 | `presets export --force` | ✅ cli/preset › export_never_replaces_a_file_unless_told_to |
| 215 | `--preset-dir` | ✅ cli/preset › an_unknown_preset_name_lists_the_ones_that_exist |
| 216 | `--journal-dir` (global) | ✅ cli/roundtrip › apply_from_a_relative_folder_undoes_from_anywhere |
| 217 | `--verbose` (global) | ✅ cli/legacy_cli › verbose_explains_what_each_switch_became |
| 218 | `--job` | ✅ cli/preset › a_preset_needs_a_directory_and_a_job_file_refuses_one |
| 219 | `--preset` (name or file) | ✅ cli/preset › a_three_operation_preset_runs_headless_and_undoes_as_one_transaction |
| 220 | `--answer` | ✅ cli/preset › an_ask_in_a_preset_can_be_answered_on_the_command_line |
| 221 | Error state: `--answer` for no slot / stdin cannot answer | ✅ cli/preset › an_answer_for_a_slot_nobody_asks_for_is_refused, an_ask_that_stdin_cannot_answer_stops_the_run |
| 222 | `--suffix`, `--scope name` | ✅ cli/roundtrip › apply_then_undo_round_trips_byte_exactly |
| 223 | `--scope extension` | ✅ cli/roundtrip › the_extension_scope_only_touches_the_extension |
| 224 | `--scope both` | ✅ cli/flags › the_both_scope_reaches_the_extension_too |
| 225 | `--folders`, `--no-files` | ✅ cli/flags › the_listing_flags_choose_files_and_folders |
| 226 | `--subfolders`, `--pattern` | ✅ cli/preset › the_listing_flags_apply_to_a_preset |
| 227 | `--list` | ✅ cli/legacy_cli › a_list_file_renames_exactly_the_files_it_names |
| 228 | `--file` | ✅ cli/roundtrip › a_file_named_twice_is_renamed_once |
| 229 | `--script-dir` | ✅ cli/legacy_cli › a_file_a_script_asked_for_is_reported_and_written |
| 230 | `--seed` | ✅ cli/flags › a_seed_makes_random_tags_reproducible |
| 231 | `--delete-list` | ✅ cli/legacy_cli › the_list_file_is_deleted_once_the_run_has_happened |
| 232 | `--allow-system-folders` (and the refusal) | ✅ cli/roundtrip › a_system_folder_is_refused_by_the_cli_too |
| 233 | Legacy `/p` (path + mask) and `/r` | ✅ cli/legacy_cli › the_scheduled_task_example_renames |
| 234 | Legacy `/l` and `/k` | ✅ cli/legacy_cli › a_successful_preview_consumes_the_list_too |
| 235 | Legacy `/f` and `/d` | ✅ cli/legacy_cli › the_include_switches_change_what_is_listed |
| 236 | Legacy `/s` | ✅ cli/flags › the_legacy_subfolder_switch_lists_nested_files |
| 237 | Legacy `/x` | ✅ cli/legacy_cli › verbose_explains_what_each_switch_became |
| 238 | Legacy `/?`, `--help`, `--version` | ✅ cli/legacy_cli › help_and_version_exit_zero |
| 239 | Exit 0 / 1 (usage) | ✅ cli/legacy_cli › success_is_zero_and_a_bad_command_line_is_one |
| 240 | Exit 2 (blocked) | ✅ cli/legacy_cli › a_conflicting_plan_exits_blocked_from_both_preview_and_apply |
| 241 | Exit 3 (failed part-way) | ✅ cli/legacy_cli › a_run_that_fails_part_way_exits_three |
| 242 | Error state: missing folder | ✅ cli/roundtrip › a_missing_folder_is_named |
| 243 | Error state: unknown preset | ✅ cli/preset › an_unknown_preset_name_lists_the_ones_that_exist |
| 244 | Non-Unicode argument | ✅ cli/roundtrip › an_argument_that_is_not_unicode_is_a_path_like_any_other |

## Updates and the installer (D249, D250)

| # | Feature | Test |
|---|---|---|
| 245 | Updates page: Check now, then Update to X starts the installer silently in the installed scope | ✅ e2e › update_to_a_newer_release_hands_over_to_the_installer |
| 246 | Updates: a portable copy is pointed at the download page; Automatic needs an installed copy | ✅ e2e › a_portable_copy_is_pointed_at_the_download_page |
| 247 | Updates: the daily check, install at exit, a checksum mismatch, the administrator prompt for an all-users install | ❌ (c) `crates/ren-gui/src/viewmodel/update.rs` › flow_tests; a day passing and the window closing are not frames a headless harness can step |
| 248 | `renameit --uninstall-cleanup` (the uninstaller's call) | ❌ (c) `crates/ren-gui/src/launch.rs` › the_uninstallers_flag_is_understood_only_on_its_own, `tests/installer_contract.rs`; the installer smoke test in `ci.yml` runs it on Windows |

## The pointer on a row (P115)

| # | Feature | Test |
|---|---|---|
| 249 | Quick double-click on a folder opens it, in the list and the grid | ✅ e2e › a_quick_double_click_opens_a_folder_in_the_list_and_the_grid |
| 250 | Slow second click renames; a Ctrl-click does not | ✅ e2e › a_slow_second_click_on_the_selected_row_renames_it |
| 251 | Columns & rows: *Double-click on a file* (Rename it / Open it) | ✅ e2e › a_quick_double_click_on_a_file_does_what_the_display_page_says |
| 252 | Columns & rows: *Double-click on a folder in Free Select* (Do nothing / Browse into it) | ✅ e2e › a_folder_in_free_select_opens_only_if_the_display_page_says_so |

## The layout (D251)

| # | Feature | Test |
|---|---|---|
| 253 | Rename says what it will do: "Rename 2 files", "Simulate 2 files", "Run on 2 files" | ✅ e2e › the_run_controls_simulate_rename_fold_the_log_and_undo; gui › a_simulated_action_only_run_says_what_it_would_have_done |
| 254 | Why Rename is unavailable is printed under it | ✅ gui › a_blocked_rename_says_why_under_the_button |
| 255 | The default window has no scrollbar | ✅ gui › the_default_window_needs_no_scrollbar |

## The right-click menu from the installer, and Open in Explorer (D252)

| # | Feature | Test |
|---|---|---|
| 256 | Open in Explorer / file manager beside ⟳ | ✅ e2e › the_folder_being_browsed_opens_in_the_file_manager |
| 257 | No ⋯ menu: ⚙ opens Settings, About is the rail's last entry (D255) | ✅ e2e › the_gear_opens_settings_and_about_is_its_last_page |
| 258 | The page's box records this user's choice | ✅ `panels/settings.rs` › the_box_records_the_users_choice |
| 259 | First start sets the menu up after an install offered it; a "no" stays | ❌ (c) `crates/ren-gui/src/app.rs` › an_install_that_offered_the_menu_sets_it_up_at_first_start and its two siblings; `start_from` is reached only from `main`, and installer-smoke covers the installer half on Windows |
