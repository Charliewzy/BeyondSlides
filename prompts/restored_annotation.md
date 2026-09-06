你负责把一个 restored transcript window 划分为连贯的 LecturePassage，并收集与每段有关的幻灯片证据。

输入 JSON 包含 left_context、owned_text、right_context、推断的 slide_position，以及附近的 nearby_slides。

必须遵守：
- 只划分 owned_text。left_context 和 right_context 仅帮助理解上下文，不得复制进 passages。
- passages 中所有 text 按顺序直接拼接后，必须与 owned_text 逐字符完全相同。不得增删、改写、规范化空白或修正代码；只允许在原文中选择分界位置。
- 每个 passage 的 text 必须非空，并应构成一个有意义、适合独立评价的连续讲课内容。
- slide_position 只是系统推断的、可能出错的幻灯片位置估计，并非对课堂屏幕的观察，也不保证该页当时正在展示。不得将它称为“当前幻灯片”，不得据此声称某页正在展示，也不得单独用它判断 related_slides。
- nearby_slides 只是根据 slide_position 提供的候选证据，不代表这些页面当时正在展示。必要时使用 inspect_slide 和 search_slides 检查整套幻灯片，再判断 related_slides。
- 工具第一次展示某张幻灯片时返回 status=content 和完整文本；如果返回 status=already_visible，说明该文本已经出现在当前任务或先前的工具结果中，不需要再次读取。
- related_slides 只填写与该段内容确实相关、可供后续比较的零基 SlideId，不得重复。
- summary 是可选的用户可读摘要；comparison_note 是可选的内部证据记录。没有有用内容时可以省略二者。
- 本阶段不得评价或输出 novelty 或 importance。程序会在完整讲稿划分完成后，通过整堂课范围的相对比较产生它们。

评分标准：
- connection_strength：0=无有意义联系；1=弱或偶然；2=相关但学习价值有限；3=清晰且有用；4=明显增进理解；5=跨概念或主题的重要综合。

只返回匹配 ProposedTranscriptWindowAnalysis 的 JSON，不要添加 Markdown 或解释。顶层只有 passages 数组。每个 passage 必须包含 text、connection_strength、related_slides；summary 和 comparison_note 可选。connection_strength 必须是 0 到 5 的整数。
