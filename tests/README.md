# 全量测试索引

版本：v1.4；更新时间：2026-11-19；适用 Markdown Studio v1.9.10。覆盖结构化文字、流程图编辑、语义分栏及公开 Windows 发布。

## 环境与执行

沿用 Cargo.lock 和现有 Rust 工具链。浏览器测试使用现有 Node + Playwright，NODE_PATH 可指定已安装依赖，MD_PREVIEW_TEST_BROWSER 可指定 Edge/Chrome；接收 EXE 的电脑不需要 Node。统一入口：`bash scripts/verify.sh`；原生测试先 `cargo build` 再 `node scripts/verify-structured-edit.mjs [真实文档路径 ...]`。所有原生编辑仅操作副本和隔离配置，debug 模式跳过文件关联，下载固定写入测试临时目录。

原生测试输出临时目录位于 target/structured-edit-*，含文档副本、原始备份、截图和实际导出的 SVG/PNG。原始输入文件最后逐一比较，必须未被修改。

## 覆盖矩阵与关键路径

| 分类 | 入口 | 验证范围 |
| --- | --- | --- |
| Rust 单元/契约 | cargo test | DOCX、资源白名单、精确备份/失败路径、会话、单实例、图片解析、页面、只读恢复和更新协议 |
| 资源完整性（TC-RICH-01） | scripts/verify-editor-assets.mjs | 固定 Vditor 4.0.1 的 12 份本地资源、大小、SHA-256、许可证 |
| 浏览器编辑（TC-RICH-02） | scripts/verify-preview-edit.mjs | 延迟离线加载、备份失败阻止进入、打开不改原文、标题/粗体/链接/代码/列表/引用/表格、新增段落/粗体、保存、搜索、未提交输入与旧保存确认竞态、当前内容打印协调、源码模式、390px |
| 原生编辑（TC-RICH-03） | scripts/verify-structured-edit.mjs | WebView2 实际回写，标题/正文首末/列表/富文本表格，元数据、公式、空格、撤销重做、IME 次序、刚输入即切换、重新打开、连续十次保存图片、原始图片引用、Mermaid 源码、原生 SVG/3x PNG 下载 |
| 原生兼容入口 | scripts/verify-webview-preview-edit.mjs | 转入 TC-RICH-03，不再运行已淘汰的普通文字标记测试 |
| 工作流（TC-FLOW-01..06） | scripts/verify-preview-workflows.mjs | 超过 500ms 的同内容通知、真正冲突保护/显式恢复，85 段含图表文档双向位置与光标定位、390px，6 类图语法的编辑/删除、图上点选、撤销、重复图定位、富文本中修改图节点 |
| 导航 | scripts/verify-anchor-navigation.mjs | 中文锚点、相对文档链接和滚动 |
| 搜索替换 | scripts/verify-desktop-search.mjs | Ctrl+F/H、替换、撤销、组合输入、自动保存和窄屏 |
| 阅读交互 | scripts/verify-desktop-reading-tools.mjs | 统计、缩放、滚动连续和分屏 |
| 图表导出 | scripts/verify-mermaid-export.mjs | 原始 SVG、3x PNG、300 PPI Word 位图及灯箱平移 |
| 图片原生加载 | scripts/verify-webview-local-images.mjs | 指定 CDP endpoint 和期望图片数时检查旧只读预览传输；本次由 TC-RICH-03 额外验证编辑模式实际图片加载 |
| 更新发布契约 | scripts/verify-sparkle-update.sh / verify-windows-self-update.sh | 更新脚本、签名/下载校验接口 |
| 便携依赖 | scripts/verify-windows-pe-dependencies.ps1 | PE 不导入 VC Runtime |
| 公开发布 | scripts/package-windows-release.ps1 / scripts/verify.sh | EXE 版本、大小、SHA-256、签名记录、许可证；中文/英文 README 及静态说明的本地链接、版本及本仓库下载入口 |
| 移动共享渲染 | mobile/scripts/verify-mobile-renderer.mjs | 只读渲染不回归；原生构建需要对应平台工具 |

关键路径：立即标记脏状态；保存/标签/关闭/确认/外部变化判断前同步 DOM；备份成功才进入；图片协议 URL 不落盘；搜索不改编辑 DOM、不退出编辑模式。旧 src/preview_edit.rs 定位筛选模块已移除，原 TC-EDIT-01..05 被上述结构化用例替代。

v1.9.10 的更新地址测试允许本仓库 Release 资产，明确拒绝上游和其他仓库的下载地址。公开后需匿名重新下载 EXE，并与发布包 SHA-256、大小比较；该网络验收不在离线 verify.sh 中执行。Windows 工作流只创建草稿，正式发布与公开下载验收由维护者完成。

## Rust 完整清单

以下共 98 个用例。前置条件和预期断言以对应测试函数为准；dump/validate 工具用例需要设置环境变量才产生相应验证物。

1. `docx::tests::adjacent_markdown_tables_stay_separate_in_word`
2. `docx::tests::crc32_matches_known_vector`
3. `docx::tests::data_url_image_is_embedded_as_media`
4. `docx::tests::dump_sample_docx_for_manual_validation`
5. `docx::tests::export_produces_valid_docx_structure`
6. `docx::tests::heading_number_prefixes_are_stripped`
7. `docx::tests::image_and_hyperlink_relationship_ids_do_not_collide`
8. `docx::tests::inline_image_stays_inside_the_surrounding_paragraph`
9. `docx::tests::logical_image_size_metadata_is_strictly_bounded`
10. `docx::tests::mermaid_fallback_uses_logical_size_not_png_sample_size`
11. `docx::tests::nested_strong_and_emphasis_keep_outer_style_after_inner_end`
12. `docx::tests::standardize_strips_bold_and_spaces`
13. `docx::tests::strip_numbers_keeps_heading_levels`
14. `docx::tests::table_alignment_keeps_markdown_and_infers_numeric_columns`
15. `docx::tests::table_content_weights_make_short_columns_narrower`
16. `docx::tests::table_hard_break_event_is_buffered_as_a_word_break`
17. `docx::tests::table_header_repeats_and_only_header_is_marked_non_splitting`
18. `docx::tests::table_images_embed_data_and_local_pngs_without_distorting_column_widths`
19. `docx::tests::table_layout_is_fixed_and_bounded_for_multiple_column_counts`
20. `docx::tests::table_preserves_links_inline_code_and_html_break_variants`
21. `docx::tests::table_style_has_margins_header_rules_and_standardize_compatibility`
22. `docx::tests::table_width_uses_custom_template_page_margins`
23. `docx::tests::template_style_is_learned_from_docx_package`
24. `docx::tests::xml_escape_filters_invalid_xml_1_0_controls_and_preserves_whitespace`
25. `editor_assets::tests::backup_preserves_original_bytes_and_does_not_modify_source`
26. `editor_assets::tests::editor_resources_are_offline_and_scoped`
27. `editor_assets::tests::missing_original_does_not_create_a_fake_backup`
28. `session::tests::closing_active_tab_selects_the_next_neighbor`
29. `session::tests::opening_the_same_path_activates_without_duplicate`
30. `session::tests::persisted_session_keeps_missing_tabs_and_active_order`
31. `session::tests::relocate_turns_untitled_into_titled_and_rejects_conflicts`
32. `session::tests::save_excludes_untitled_tabs_and_reindexes_active`
33. `session::tests::untitled_tab_is_scratch_only_and_never_missing`
34. `single_instance::tests::candidate_ports_avoid_the_regressed_windows_reserved_block`
35. `single_instance::tests::free_candidate_acquires_primary_without_probe_delay`
36. `single_instance::tests::instance_key_and_candidate_port_are_stable`
37. `single_instance::tests::instance_key_does_not_change_when_config_directory_is_created`
38. `single_instance::tests::large_path_lists_are_chunked_without_dropping_documents`
39. `single_instance::tests::loopback_forwarding_waits_for_an_exact_ack`
40. `single_instance::tests::request_codec_rejects_bad_magic_and_oversized_frames`
41. `single_instance::tests::request_codec_round_trips_unicode_paths_and_edit_mode`
42. `tests::close_flow_acknowledges_when_nothing_to_save`
43. `tests::closing_the_last_editing_tab_restores_the_empty_preview`
44. `tests::dollar_math_is_protected_from_markdown_emphasis`
45. `tests::dollar_math_is_protected_from_markdown_escapes`
46. `tests::double_equals_highlight_renders_mark_without_touching_code`
47. `tests::dump_demo_page_when_requested`
48. `tests::dump_mermaid_page_when_requested`
49. `tests::dump_page_for_js_syntax_check`
50. `tests::dump_preview_edit_page_when_requested`
51. `tests::empty_state_exposes_open_and_recent_files`
52. `tests::external_change_protection_uses_dirty_state_from_either_side`
53. `tests::file_urls_only_open_existing_supported_documents`
54. `tests::file_watch_scope_is_parent_directory`
55. `tests::finder_action_parses_encoded_folder_and_kind`
56. `tests::finder_create_uses_non_conflicting_markdown_name`
57. `tests::generated_heading_ids_are_unique_and_keep_explicit_ids`
58. `tests::generated_heading_ids_support_cjk_anchor_links`
59. `tests::github_alerts_render_as_markdown_alert_blockquotes`
60. `tests::help_flags_are_recognized`
61. `tests::horizontal_rules_and_setext_headings_are_not_front_matter`
62. `tests::large_chinese_and_percent_encoded_image_paths_do_not_expand_html`
63. `tests::linux_nvidia_compat_env_only_sets_dmabuf_when_unconfigured`
64. `tests::local_image_mime_type_accepts_common_jpeg_extensions`
65. `tests::local_image_protocol_rejects_unregistered_assets`
66. `tests::local_image_transport_url_matches_the_platform_webview_origin`
67. `tests::local_image_urls_survive_the_next_document_render`
68. `tests::local_images_reject_remote_urls_and_network_shares`
69. `tests::local_images_support_exact_parent_backslash_absolute_and_file_urls`
70. `tests::local_relative_images_are_streamed_from_markdown_directory`
71. `tests::local_relative_images_keep_original_src_when_unreadable`
72. `tests::new_markdown_path_keeps_markdown_extensions_and_replaces_other_extensions`
73. `tests::page_blocks_native_preview_reload_paths`
74. `tests::page_expands_multi_column_tables`
75. `tests::page_includes_about_popover_tab_menu_and_pdf_export`
76. `tests::page_includes_mermaid_lightbox_viewer`
77. `tests::page_includes_outline_sidebar_navigation`
78. `tests::page_separates_new_file_from_open_and_debounces_autosave`
79. `tests::save_failure_dialog_maps_save_discard_and_cancel`
80. `tests::self_write_suppression_checks_disk_content_not_only_time`
81. `tests::sibling_file_events_do_not_reload_current_file`
82. `tests::theme_choice_parses_menu_values`
83. `tests::update_download_urls_are_allowed`
84. `tests::update_release_selection_ignores_older_and_prerelease_versions`
85. `tests::update_versions_compare_semver_tags`
86. `tests::utc_now_string_formats_civil_time`
87. `tests::validate_local_image_document_when_requested`
88. `tests::version_flags_are_recognized`
89. `tests::vim_style_target_rewrite_events_reload_current_file`
90. `tests::webview_startup_failure_detects_missing_runtime_hresult`
91. `tests::webview_startup_failure_keeps_unrelated_errors_generic`
92. `tests::windows_association_plan_quotes_paths_and_keeps_legacy_default_compatible`
93. `tests::windows_association_registration_requires_marker_and_every_command`
94. `tests::windows_association_shell_path_removes_verbatim_prefixes`
95. `tests::yaml_front_matter_accepts_dots_and_escapes_html`
96. `tests::yaml_front_matter_keeps_metadata_readable_without_blank_lines`
97. `source_map::tests::maps_cjk_emoji_crlf_and_metadata_to_textarea_offsets`
98. `source_map::tests::annotates_list_items_and_rows_without_extra_dom_wrappers`

## 最近执行结果

v1.9.10（2026-11-19）：Rust 98/0，fmt、release 构建、统一门禁、PE 与 CLI 通过；本仓库更新地址和拒绝上游下载的契约通过。隔离 WebView2 原生工作流与结构化 fixture 回归通过，含冲突保护、语义分栏、节点编辑/删除/撤销、图片连续保存、SVG 314px / PNG 942px 下载。打包脚本实跑通过，ZIP 完整性、EXE 字节一致、四份许可证及全部 SHA-256 已核验。真实截图使用合成示例，不含业务文档。公开下载验收在正式发布后执行。

历史 v1.9.9 Git 归档前检查：从当时 CI 文件提取真实校验/打包脚本，在本地临时目录验证部署文档查找、错误 tag 版本拒绝、表格/旧项目符号两种元数据更新及四份许可证复制；检查通过。当时尚未执行远程 push 或公开发布。归档规则见 `release/README.md`。

v1.9.9：Rust 98/0，统一门禁、发布构建、PE 与 CLI 通过。新增原生工作流覆盖真正冲突/同内容通知、分栏双向精确锚点和 Ctrl+H、390px 与打印布局，节点点选/编辑/删除/撤销及富文本回写通过。两份真实方案共6张抽样流程图，6次标签修改及6次节点删除校验通过。测试证据在 target/preview-workflows-*，不随程序发布。

2026-11-19：结构化浏览器与 Windows 原生真实文档副本回归通过；原生 SVG 自然宽度314px，PNG宽度942px。真实v1.4.0编辑 DOM 含85标题、68段落、114列表项、914单元格，原始方案含63标题、48段落、66列表项、756单元格；选择首部/中部/末尾目标并实际回写，原件未变。最终结果记录在v1.9.9部署说明。

## 未验证与边界

物理中文输入法人工输入、干净 Windows 11 VM、macOS/Linux 原生编辑待实机验证。IME 自动化通过组合事件检查保存次序，不能替代真实输入法。自定义 HTML 可能显示为源码块；Markdown 排版会规范化；撤销栈不跨模式/标签保留。图形编辑只覆盖 flowchart/graph 节点，不覆盖其他图类型、节点拖拽或边文字编辑；无法安全处理的扩展语法保持源码编辑。原生只读失败对话框沿用既有实现，本轮没有重新手工操作该对话框。

