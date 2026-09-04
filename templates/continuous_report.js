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
  let viewingFrame;
  let activeAudioEnd;

  for (const passage of passages) {
    const matches = passagesBySlideId.get(passage.dataset.slidePosition) || [];
    matches.push(passage);
    passagesBySlideId.set(passage.dataset.slidePosition, matches);
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

    activeAudioEnd = end;
    audio.currentTime = start;
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
    audio.pause();
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
    range.textContent = `来源片段 #${passage.dataset.sourceStart}–${passage.dataset.sourceEnd} · ${passage.dataset.time}`;
    details.textContent = `对齐页 ${passage.dataset.slideNumber} · 重要性 ${passage.dataset.importance} · 新颖度 ${passage.dataset.novelty} · 连接强度 ${passage.dataset.connection}`;

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
    slide.addEventListener("click", () => inspectSlide(slide));
    slide.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        inspectSlide(slide);
      }
    });
  }

  if (rail) {
    rail.addEventListener("scroll", scheduleViewingSlideUpdate, { passive: true });
    window.addEventListener("resize", scheduleViewingSlideUpdate);
  }
  if (audio) {
    audio.addEventListener("timeupdate", () => {
      if (activeAudioEnd !== undefined && audio.currentTime >= activeAudioEnd) {
        audio.pause();
        audio.currentTime = activeAudioEnd;
        activeAudioEnd = undefined;
        audioStatus.textContent = "已播放所选讲稿段落的音频";
      }
    });
  }
  if (passages.length > 0) {
    inspectPassage(passages[0], "auto");
  } else {
    updateViewingSlide();
  }
})();
