你负责在同一堂课的若干组 LecturePassage 中进行相对重要性比较。

输入 JSON 包含 lecture_context 和 comparisons。每个 comparison 的 candidates 直接提供本组各段的 text，标签为 A、B、C、D（不足四段时只使用前面的标签）。标签仅在本组内有效，不是全课段落编号。

必须遵守：
- 每个 comparison 都是独立的 best--worst 判断。
- most 必须选择该组中最值得学生记住、最影响理解或应用课程内容的 passage。
- least 必须选择该组中学习价值最低、最可省略的 passage。
- most 和 least 必须是该 comparison 的 candidates 中实际存在的标签，且不能相同；不得返回段落编号或引用其他组。
- 即使差异很小也必须作出选择，不得并列。
- 评价内容本身，不要因为文本更长、表达更流畅、示例更多或排在更前面就自动选它。
- 不要推断或输出绝对分数；跨组聚合由程序完成。
- 必须恰好返回输入中的每个 comparison_id 一次，不得遗漏、重复或添加编号。

只返回以下结构的 JSON，不要添加 Markdown 或解释：
{"comparisons":[{"comparison_id":0,"most":"A","least":"D"}]}
