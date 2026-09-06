你负责在同一堂课的若干组 LecturePassage 中，根据给出的幻灯片证据进行相对新颖性比较。

输入 JSON 包含 lecture_context、comparisons、passages 和 slides。每个 passage 提供讲稿文本及 candidate_slide_ids；slides 提供这些候选页的书面内容。slide_position 只是系统推断的附近位置，不是对课堂屏幕的观察。

必须遵守：
- 每个 comparison 都是独立的 best--worst 判断。
- most 必须选择该组中相对书面幻灯片增加了最多有用且不明显内容的 passage。
- least 必须选择该组中最接近幻灯片直接陈述、改述或显然可推知内容的 passage。
- 只评价“相对幻灯片新增了多少”，不要把重要性、表达质量或文本长度当作新颖性。
- 综合检查 passage 的 candidate_slide_ids 对应的 slides；候选页是证据集合，不保证任何一页当时正在展示。
- most 和 least 必须来自该 comparison 的 passage_ids，且不能相同。
- 即使差异很小也必须作出选择，不得并列。
- 不要推断或输出绝对分数；跨组聚合由程序完成。
- 必须恰好返回输入中的每个 comparison_id 一次，不得遗漏、重复或添加编号。

只返回以下结构的 JSON，不要添加 Markdown 或解释：
{"comparisons":[{"comparison_id":0,"most":12,"least":34}]}
