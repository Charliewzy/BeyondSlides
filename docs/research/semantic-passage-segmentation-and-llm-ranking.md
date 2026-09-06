# Semantic passage segmentation and LLM score calibration

Date: 2026-09-04

## Recommendation

Treat passage boundaries and semantic scores as two different inference tasks.
First partition the restored transcript into coherent, independently evaluable
lecture passages without asking whether any passage is important or novel.
Then score the fixed passages against the written source. For calibration, test
relative judgments or best--worst scaling on a human-labeled sample; do not
assume that expanding the integer scale from `0..5` to `0..10` creates useful
resolution.

The evidence below supports this architecture and makes a `0..10` scale an
unpromising default. It does **not** establish the correct passage granularity,
rubric, or score distribution for BeyondSlides. Those are project-specific
questions that require human labels from the target Chinese lecture domain.

## Direct evidence: semantic segmentation is its own task

Malioutov and Barzilay define spoken-lecture segmentation as partitioning a
transcript into a linear sequence of topically coherent segments. Their
unsupervised minimum-cut model optimizes similarity within segments and
dissimilarity across segments; it does not use importance, novelty, or summary
worthiness as boundary labels. On 33 undergraduate physics lectures, the ASR
transcripts had a 19.4% word-error rate. The model's Pk error rose only from
0.298 on manual transcripts to 0.322 on ASR, a 7.8% relative increase. This is
direct evidence that coherence-based boundary inference can remain useful in
noisy lecture transcripts. [Malioutov and Barzilay, *Minimum Cut Model for
Spoken Lecture Segmentation* (ACL 2006)](https://aclanthology.org/P06-1004/)

That study also exposes a granularity problem rather than a single objectively
correct partition. Four human/reference segmentations of the same first ten
physics lectures averaged 6.6, 8.9, 18.4, and 13.8 segments per lecture, with
pairwise Pk disagreement from 0.24 to 0.42. The authors conclude that multiple
granularities are acceptable. Their segments represent high-level subtopics,
so the result does not directly validate BeyondSlides' much finer lecture
passages; it shows why an explicit target-granularity policy is necessary.
[Malioutov and Barzilay (2006)](https://aclanthology.org/P06-1004/)

More recent work framed around ASR transcripts retains the same task separation
while replacing lexical counts with learned semantic representations. M3Seg
defines boundaries so that thematic meaning is cohesive inside a segment and
different across adjacent segments. On two public ASR meeting datasets, its
unsupervised objective
improved the reported segmentation metrics by 18%--37% over prior systems.
This supports semantic coherence as a boundary signal independent of salience,
although meetings are not lectures and its segment scale is again coarser than
BeyondSlides' evaluation units. Importantly, the experiment used
human-corrected reference transcripts because the original AMI and ICSI ASR
outputs appeared unavailable; it did not establish robustness to raw ASR noise.
[Wang et al., *M3Seg: A Maximum-Minimum Mutual
Information Paradigm for Unsupervised Topic Segmentation in ASR Transcripts*
(EMNLP 2023)](https://aclanthology.org/2023.emnlp-main.492/)

Koshorek et al. formulate text segmentation directly as binary decisions
between adjacent sentences in a contiguous document and show that the target
granularity can be changed by choosing a different level of a known document
hierarchy. This is further evidence that boundaries are a structural prediction
with a separately chosen resolution, not a by-product of assigning scalar
quality scores. Their training and main evaluation data are Wikipedia, not
spoken lectures. [Koshorek et al., *Text Segmentation as a Supervised Learning
Task* (NAACL 2018)](https://aclanthology.org/N18-2075/)

## Direct evidence: a wider absolute scale is not better calibration

Liusie et al. directly compare simple `1..10` pointwise prompts with pairwise
LLM judgments on summarization, dialogue, podcast, and data-to-text evaluation.
For the moderate Flan-T5 and Llama-2 models studied, comparative assessment
outperformed simple prompt scoring in nearly all settings and produced much
better item-level resolution for podcast summaries. The study therefore gives
no support to the idea that ten integer choices alone solve absolute scoring.
However, its pairwise method can require `N(N-1)` ordered comparisons, and some
configurations selected the first-presented candidate as often as 80% of the
time. The authors improve results by estimating and correcting that positional
bias. [Liusie et al., *LLM Comparative Assessment* (EACL
2024)](https://aclanthology.org/2024.eacl-long.8/)

Huang et al. compare ChatGPT judgments with human ratings of explanation
clarity and informativeness using binary, ternary, and seven-point scales.
Across 300 items with 900 newly collected human annotations, ChatGPT aligned
better with people on the coarser scales; paired comparison and prompts with
semantically similar examples also improved alignment. This is task-specific
evidence against assuming that additional scale points provide additional
valid information. [Huang et al., *ChatGPT Rates Natural Language Explanation
Quality like Humans: But on Which Scales?* (LREC-COLING
2024)](https://aclanthology.org/2024.lrec-main.277/)

Licht et al. study scalar text measurement with `1..9` pointwise scales across
three political-language datasets and five open Llama/Qwen variants. Directly
generated integer scores exhibit "heaping": probability mass concentrates on
a few arbitrary numeric values and different models produce substantially
different distributions. Aggregated pairwise comparisons outperform the raw
integer outputs, but a probability-weighted mean over all score-token
probabilities performs better still in their experiments. Thus relative ranking
is not universally best; access to calibrated token probabilities can rescue a
pointwise method. The paper explicitly limits generalization beyond its English
political constructs, and closed APIs may not expose enough log probabilities
to reproduce that method. [Licht et al., *Measuring Scalar Constructs in Social
Science with LLMs* (EMNLP
2025)](https://aclanthology.org/2025.emnlp-main.1635/)

Four-item best--worst scaling offers a cheaper relative protocol than comparing
every pair. In an emotion-intensity experiment, GPT-3.5 chose the most and least
intense items in each four-text set; these labels were more reliable than
direct rating scales or exhaustive pairwise comparisons, and used `2N` tuples
instead of the much larger all-pairs set. The authors explicitly caution that
this result still needs validation on other regression tasks and longer texts.
[Bagdon et al., *Automatic Best--Worst-Scaling Annotations for Emotion
Intensity Modeling* (NAACL
2024)](https://aclanthology.org/2024.naacl-long.439/)

Comparative judgment has its own failure modes. Jeong et al. find that pairwise
LLM evaluation can amplify preferences for superficial features such as
verbosity and authoritative tone. Their PRePair method first reasons about each
candidate independently and then compares them, outperforming pure pointwise
evaluation on a standard benchmark while reducing pairwise bias on an
adversarial benchmark. This is evidence for a hybrid, not for replacing every
absolute judgment with an unqualified pairwise vote. [Jeong et al., *The
Comparative Trap* (BlackboxNLP
2025)](https://aclanthology.org/2025.blackboxnlp-1.5/)

## Proposed BeyondSlides synthesis

The following is a project proposal inferred from the studies above, not a
configuration any cited paper tested:

1. **Boundary pass.** Give the model authoritative restored transcript text and
   neighboring text only. Ask it to partition the owned region by semantic
   coherence and independent evaluability. Do not expose or request novelty,
   connection strength, importance, or related-slide judgments in this pass.
   Specify soft minimum/maximum lengths plus examples for the desired fine
   lecture-passage granularity; topic-transition instructions alone would
   likely produce units far coarser than the current product needs. Continue to
   project copied boundary proposals onto authoritative restored text, as
   required by [ADR 0003](../adr/0003-project-proposed-passages-onto-restored-text.md);
   separating the passes does not weaken the provenance model established by
   [ADR 0002](../adr/0002-annotate-restored-text-with-coarse-source-provenance.md)
   and ADR 0003.
2. **Evidence and pointwise pass.** Score the now-fixed lecture passages against
   the written source with the existing `0..5` schema. Strengthen behavioral
   anchors and provide labeled examples spanning each level. The labels remain
   interpretable product judgments, while boundary choices can no longer expand
   or merge text to make a score easier to assign.
3. **Calibration experiment.** On one human-reviewed, stratified passage set,
   compare (a) the current `0..5` prompt, (b) anchored/few-shot `0..5`, (c)
   `0..10`, and (d) counterbalanced relative judgments. Best--worst sets of four
   are a more economical first relative treatment than all pairs. Randomize or
   reverse presentation order, repeat a subset to measure stability, and keep
   novelty comparisons grounded in the same written-source evidence.
4. **Preserve the distinction between rank and level.** Pairwise or best--worst
   results supply an ordinal lecture-wide ranking. They do not by themselves
   say that a passage is an absolute `4` or qualifies as an oral addition. Map
   latent ranks back to product levels only against human-anchored thresholds;
   otherwise expose percentiles as relative navigation aids while retaining the
   rubric score for interpretation.

Evaluate boundaries separately from scores. Boundary review should ask whether
each passage is coherent and independently evaluable at the intended
granularity. Score evaluation should measure agreement with human ordinal
labels, rank correlation, repeated-run stability, and confusion between
adjacent levels. A balanced-looking histogram is not a validity target: the
lecture may genuinely contain many important passages. The current concentrated
distribution is evidence that calibration needs testing, not evidence that the
correct target distribution is uniform.

## Bottom line

No cited study directly compares `0..5` with `0..10` for Chinese lecture-passage
importance, novelty, or connection strength. The closest evidence points away
from widening the scale: simple `1..10` prompting can have poor resolution,
`1..9` outputs heap on preferred numbers, and a seven-point scale aligned less
well with humans than coarser alternatives in one text-quality study. The next
high-information change is therefore a boundary-only pass plus human-calibrated
rubric/ranking evaluation, not a production switch to `0..10`.
