(() => {
  const passages = [...document.querySelectorAll("[data-passage]")];
  const range = document.querySelector("[data-inspector-range]");
  const details = document.querySelector("[data-inspector-details]");

  function inspectPassage(passage) {
    for (const candidate of passages) {
      candidate.classList.toggle("selected", candidate === passage);
    }
    range.textContent = `来源片段 #${passage.dataset.sourceStart}–${passage.dataset.sourceEnd} · ${passage.dataset.time}`;
    details.textContent = `重要性 ${passage.dataset.importance} · 新颖度 ${passage.dataset.novelty} · 连接强度 ${passage.dataset.connection}`;
  }

  for (const passage of passages) {
    passage.addEventListener("click", () => inspectPassage(passage));
    passage.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        inspectPassage(passage);
      }
    });
  }
})();
