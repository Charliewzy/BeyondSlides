(() => {
  const passages = [...document.querySelectorAll("[data-passage]")];
  const range = document.querySelector("[data-inspector-range]");
  const details = document.querySelector("[data-inspector-details]");
  const rail = document.querySelector("[data-slide-rail]");
  const slides = [...document.querySelectorAll("[data-slide]")];
  const slidesById = new Map(slides.map(slide => [slide.dataset.slideId, slide]));
  const passagesBySlideId = new Map();
  const alignedSlideLabel = document.querySelector("[data-aligned-slide-label]");
  const viewingSlideLabel = document.querySelector("[data-viewing-slide-label]");
  const audio = document.querySelector("[data-lecture-audio]");
  const audioStatus = document.querySelector("[data-audio-status]");
  const playbackModeButtons = [...document.querySelectorAll("[data-playback-mode]")];
  const scoreThresholds = [...document.querySelectorAll("[data-score-threshold]")];
  const audioIntervals = passages.map(passage => ({
    passage,
    start: Number(passage.dataset.audioStartMs) / 1000,
    end: Number(passage.dataset.audioEndMs) / 1000,
    precise: passage.dataset.audioBasis === "timed_tokens",
  }));
  let viewingFrame;
  let activeAudioEnd;
  let boundedPassage;
  let passageSeekTarget;
  let audioCurrentPassage;
  let playbackMode = "passage";

  for (const passage of passages) {
    const matches = passagesBySlideId.get(passage.dataset.slidePosition) || [];
    matches.push(passage);
    passagesBySlideId.set(passage.dataset.slidePosition, matches);
  }

  function thresholdLabel(topPercent) {
    if (topPercent === 0) return "关闭";
    if (topPercent === 100) return "全部";
    return `前 ${topPercent}%`;
  }

  function passagePercentile(passage, metric) {
    const percentileText = passage.dataset[`${metric}Percentile`];
    const percentile = Number(percentileText);
    if (percentileText !== "" && Number.isFinite(percentile)) return percentile;

    // Reports created before comparative percentiles were persisted can still
    // use their display levels. New reports always take the branch above.
    const level = Number(passage.dataset[metric]);
    return Number.isFinite(level) ? Math.max(0, Math.min(100, (level - 1) * 20)) : 0;
  }

  function cutoffIncludesTie(metric, emphasizedPassages) {
    if (emphasizedPassages.length === 0 || emphasizedPassages.length === passages.length) return false;
    const cutoff = Math.min(...emphasizedPassages.map(passage => passagePercentile(passage, metric)));
    return passages.filter(passage => passagePercentile(passage, metric) === cutoff).length > 1;
  }

  function updateScoreWidgetSummary() {
    const summary = document.querySelector("[data-score-widget-summary]");
    if (!summary) return;
    const labels = new Map(scoreThresholds.map(input => [
      input.dataset.scoreThreshold,
      thresholdLabel(Number(input.value)),
    ]));
    summary.textContent = `粗体 ${labels.get("importance")} · 下划线 ${labels.get("novelty")}`;
  }

  function passageNearestViewportCenter() {
    const viewportCenter = window.innerHeight / 2;
    let nearest;
    let nearestDistance = Number.POSITIVE_INFINITY;
    for (const passage of passages) {
      const bounds = passage.getBoundingClientRect();
      const distance = Math.abs(bounds.top + bounds.height / 2 - viewportCenter);
      if (distance < nearestDistance) {
        nearest = passage;
        nearestDistance = distance;
      }
    }
    return nearest;
  }

  function applyScoreThreshold(input, preserveScrollPosition) {
    const metric = input.dataset.scoreThreshold;
    const topPercent = Number(input.value);
    const percentileThreshold = 100 - topPercent;
    const anchor = preserveScrollPosition ? passageNearestViewportCenter() : undefined;
    const anchorTop = anchor?.getBoundingClientRect().top;
    const emphasizedPassages = [];

    for (const passage of passages) {
      const emphasized = topPercent > 0
        && passagePercentile(passage, metric) >= percentileThreshold;
      passage.classList.toggle(`${metric}-emphasized`, emphasized);
      if (emphasized) emphasizedPassages.push(passage);
    }

    const label = thresholdLabel(topPercent);
    const actualPercent = passages.length === 0
      ? 0
      : emphasizedPassages.length / passages.length * 100;
    const tieNote = cutoffIncludesTie(metric, emphasizedPassages) ? "，含并列" : "";
    document.querySelector(`[data-threshold-output="${metric}"]`).textContent = label;
    document.querySelector(`[data-threshold-count="${metric}"]`).textContent =
      `实际 ${emphasizedPassages.length}/${passages.length} 段（${actualPercent.toFixed(1)}%${tieNote}）`;
    input.setAttribute("aria-valuetext", label);
    updateScoreWidgetSummary();

    if (anchor && anchorTop !== undefined) {
      window.scrollBy(0, anchor.getBoundingClientRect().top - anchorTop);
    }
  }

  function setupCollapsibleWidget(widget) {
    const toggle = widget.querySelector("[data-widget-toggle]");
    const name = widget.dataset.collapsibleWidget;
    if (!toggle || !name) return;
    const storageKey = `beyond-slides.report-widget.${name}.v1`;

    function setCollapsed(collapsed, persist) {
      widget.classList.toggle("is-collapsed", collapsed);
      toggle.textContent = collapsed ? "展开" : "收起";
      toggle.setAttribute("aria-expanded", String(!collapsed));
      toggle.setAttribute("aria-label", `${collapsed ? "展开" : "收起"}${name === "score-controls" ? "阅读标记设置" : "段落信息"}`);
      if (persist) {
        try { localStorage.setItem(storageKey, collapsed ? "collapsed" : "expanded"); } catch (_) {}
      }
    }

    let collapsed = false;
    try { collapsed = localStorage.getItem(storageKey) === "collapsed"; } catch (_) {}
    setCollapsed(collapsed, false);
    toggle.addEventListener("click", () => setCollapsed(!widget.classList.contains("is-collapsed"), true));
  }

  function updateViewingSlide() {
    viewingFrame = undefined;
    if (!rail || slides.length === 0) return;

    const railBounds = rail.getBoundingClientRect();
    const railCenter = railBounds.top + railBounds.height / 2;
    let viewingSlide = slides[0];
    let closestDistance = Number.POSITIVE_INFINITY;

    for (const slide of slides) {
      const bounds = slide.getBoundingClientRect();
      const distance = Math.abs(bounds.top + bounds.height / 2 - railCenter);
      if (distance < closestDistance) {
        viewingSlide = slide;
        closestDistance = distance;
      }
    }

    for (const slide of slides) {
      slide.classList.toggle("viewing", slide === viewingSlide);
    }
    viewingSlideLabel.textContent = `浏览页 ${viewingSlide.dataset.slideNumber}`;
  }

  function scheduleViewingSlideUpdate() {
    if (viewingFrame === undefined) {
      viewingFrame = requestAnimationFrame(updateViewingSlide);
    }
  }

  function centerSlide(slide, behavior) {
    const railBounds = rail.getBoundingClientRect();
    const slideBounds = slide.getBoundingClientRect();
    rail.scrollTo({
      top: rail.scrollTop + slideBounds.top - railBounds.top
        - (rail.clientHeight - slideBounds.height) / 2,
      behavior,
    });
    scheduleViewingSlideUpdate();
  }

  function playPassageAudio(passage) {
    if (!audio) return;
    const start = Number(passage.dataset.audioStartMs) / 1000;
    const end = Number(passage.dataset.audioEndMs) / 1000;
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) return;

    boundedPassage = playbackMode === "passage" ? passage : undefined;
    activeAudioEnd = boundedPassage ? end : undefined;
    passageSeekTarget = start;
    audio.currentTime = start;
    setAudioCurrentPassage(passage, false);
    audioStatus.textContent = `正在播放 ${passage.dataset.time}`;
    const playback = audio.play();
    if (playback) {
      playback.catch(() => {
        audioStatus.textContent = `音频已定位到 ${passage.dataset.time}；点击播放按钮开始`;
      });
    }
  }

  function pausePassageAudio() {
    if (!audio) return;
    activeAudioEnd = undefined;
    boundedPassage = undefined;
    passageSeekTarget = undefined;
    audio.pause();
    setAudioCurrentPassage(undefined, false);
  }

  function findAudioPassage(time) {
    if (!Number.isFinite(time)) return;
    let match;
    for (const interval of audioIntervals) {
      if (!Number.isFinite(interval.start) || !Number.isFinite(interval.end)
          || time < interval.start || time >= interval.end) {
        continue;
      }
      if (!match
          || (interval.precise && !match.precise)
          || (interval.precise === match.precise && interval.start > match.start)) {
        match = interval;
      }
    }
    return match?.passage;
  }

  function passageIsVisible(passage) {
    const bounds = passage.getBoundingClientRect();
    const margin = Math.min(120, window.innerHeight / 4);
    return bounds.top >= margin && bounds.bottom <= window.innerHeight - margin;
  }

  function showPassageDetails(passage, prefix = "") {
    const label = `来源片段 #${passage.dataset.sourceStart}–${passage.dataset.sourceEnd} · ${passage.dataset.time}`;
    range.textContent = prefix ? `${prefix} · ${label}` : label;
    const importancePercentile = passage.dataset.importancePercentile;
    const noveltyPercentile = passage.dataset.noveltyPercentile;
    const importance = importancePercentile
      ? `${passage.dataset.importance}（全讲 ${importancePercentile}%）`
      : passage.dataset.importance;
    const novelty = noveltyPercentile
      ? `${passage.dataset.novelty}（全讲 ${noveltyPercentile}%）`
      : passage.dataset.novelty;
    details.textContent = `对齐页 ${passage.dataset.slideNumber} · 重要性 ${importance} · 新颖度 ${novelty}`;
  }

  function setAudioCurrentPassage(passage, forceScroll) {
    const changed = passage !== audioCurrentPassage;
    audioCurrentPassage = passage;
    for (const candidate of passages) {
      candidate.classList.toggle("audio-current", candidate === passage);
    }
    if (!passage) return;

    showPassageDetails(passage, audio && audio.paused ? "当前音频" : "正在播放");
    if (audioStatus) {
      audioStatus.textContent = audio && audio.paused
        ? `音频已定位到 ${passage.dataset.time}`
        : `正在播放 ${passage.dataset.time}`;
    }
    const alignedSlide = slidesById.get(passage.dataset.slidePosition);
    if (alignedSlide && (changed || forceScroll)) {
      activateSlide(alignedSlide);
      centerSlide(alignedSlide, "smooth");
    }
    if (forceScroll || (changed && audio && !audio.paused && !passageIsVisible(passage))) {
      passage.scrollIntoView({ behavior: "smooth", block: "center" });
    }
  }

  function synchronizeAudioPosition(forceScroll = false) {
    if (!audio || !Number.isFinite(audio.currentTime)) return;
    setAudioCurrentPassage(findAudioPassage(audio.currentTime), forceScroll);
  }

  function setPlaybackMode(mode) {
    playbackMode = mode;
    for (const button of playbackModeButtons) {
      button.setAttribute("aria-pressed", String(button.dataset.playbackMode === mode));
    }
    activeAudioEnd = undefined;
    boundedPassage = undefined;
  }

  function activateSlide(slide) {
    const slideId = slide.dataset.slideId;
    const matchedPassages = passagesBySlideId.get(slideId) || [];
    for (const passage of passages) {
      passage.classList.toggle("slide-aligned", passage.dataset.slidePosition === slideId);
    }
    for (const candidate of slides) {
      const aligned = candidate === slide;
      candidate.classList.toggle("aligned", aligned);
      candidate.setAttribute("aria-pressed", String(aligned));
    }
    alignedSlideLabel.textContent = `对齐页 ${slide.dataset.slideNumber}`;
    return matchedPassages;
  }

  function previewSlidePassages(slide, previewed) {
    const matchedPassages = passagesBySlideId.get(slide.dataset.slideId) || [];
    for (const passage of matchedPassages) {
      passage.classList.toggle("slide-hover-preview", previewed);
    }
  }

  function inspectSlide(slide) {
    for (const passage of passages) {
      passage.classList.remove("selected");
    }
    pausePassageAudio();
    const matchedPassages = activateSlide(slide);
    centerSlide(slide, "smooth");

    if (matchedPassages.length === 0) {
      range.textContent = `幻灯片 ${slide.dataset.slideNumber} · 没有对齐讲稿`;
      details.textContent = "该页没有被推断为任何讲稿段落的时间位置。";
      if (audioStatus) audioStatus.textContent = "选择具体讲稿段落可播放音频";
      return;
    }

    const first = matchedPassages[0];
    const last = matchedPassages[matchedPassages.length - 1];
    const firstStart = first.dataset.time.split("–")[0];
    const lastEnd = last.dataset.time.split("–").at(-1);
    range.textContent = `幻灯片 ${slide.dataset.slideNumber} · 对齐 ${matchedPassages.length} 个段落`;
    details.textContent = `讲稿时间 ${firstStart}–${lastEnd} · 点击具体段落可查看评分`;
    if (audioStatus) audioStatus.textContent = "已定位到对齐讲稿；点击具体段落播放音频";
    first.scrollIntoView({ behavior: "smooth", block: "center" });
  }

  function inspectPassage(passage, scrollBehavior = "smooth", playAudio = false) {
    for (const candidate of passages) {
      candidate.classList.toggle("selected", candidate === passage);
    }
    showPassageDetails(passage);

    if (playAudio) playPassageAudio(passage);

    if (!rail) return;
    const alignedSlide = slidesById.get(passage.dataset.slidePosition);
    if (!alignedSlide) return;
    activateSlide(alignedSlide);
    centerSlide(alignedSlide, scrollBehavior);
  }

  for (const passage of passages) {
    passage.addEventListener("click", () => inspectPassage(passage, "smooth", true));
    passage.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        inspectPassage(passage, "smooth", true);
      }
    });
  }

  for (const slide of slides) {
    slide.addEventListener("pointerenter", () => previewSlidePassages(slide, true));
    slide.addEventListener("pointerleave", () => previewSlidePassages(slide, false));
    slide.addEventListener("click", () => inspectSlide(slide));
    slide.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        inspectSlide(slide);
      }
    });
  }

  for (const button of playbackModeButtons) {
    button.addEventListener("click", () => setPlaybackMode(button.dataset.playbackMode));
  }

  for (const input of scoreThresholds) {
    applyScoreThreshold(input, false);
    input.addEventListener("input", () => applyScoreThreshold(input, true));
  }
  for (const widget of document.querySelectorAll("[data-collapsible-widget]")) {
    setupCollapsibleWidget(widget);
  }

  if (rail) {
    rail.addEventListener("scroll", scheduleViewingSlideUpdate, { passive: true });
    window.addEventListener("resize", scheduleViewingSlideUpdate);
  }
  if (audio) {
    audio.addEventListener("timeupdate", () => {
      if (activeAudioEnd !== undefined && audio.currentTime >= activeAudioEnd) {
        const passage = boundedPassage;
        audio.pause();
        const passageStart = passage ? Number(passage.dataset.audioStartMs) / 1000 : 0;
        passageSeekTarget = Math.max(passageStart, activeAudioEnd - 0.001);
        audio.currentTime = passageSeekTarget;
        activeAudioEnd = undefined;
        boundedPassage = undefined;
        setAudioCurrentPassage(passage, false);
        audioStatus.textContent = "已播放所选讲稿段落的音频";
        return;
      }
      synchronizeAudioPosition();
    });
    audio.addEventListener("seeking", () => {
      const isPassageSeek = passageSeekTarget !== undefined
        && Math.abs(audio.currentTime - passageSeekTarget) < 0.05;
      if (!isPassageSeek) {
        activeAudioEnd = undefined;
        boundedPassage = undefined;
      }
      synchronizeAudioPosition();
    });
    audio.addEventListener("seeked", () => {
      passageSeekTarget = undefined;
      synchronizeAudioPosition(true);
    });
    audio.addEventListener("play", () => {
      synchronizeAudioPosition();
    });
  }
  if (passages.length > 0) {
    inspectPassage(passages[0], "auto");
  } else {
    updateViewingSlide();
  }
})();
