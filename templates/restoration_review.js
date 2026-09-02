(() => {
  const buttons = [...document.querySelectorAll("[data-review-filter]")];
  const windows = [...document.querySelectorAll("[data-review-window]")];
  const visibleCount = document.querySelector("[data-visible-window-count]");

  function applyFilter(filter) {
    let count = 0;
    for (const window of windows) {
      const visible = filter === "all"
        || (filter === "repaired" && window.dataset.hasRepair === "true")
        || (filter === "omissions" && window.dataset.hasOmission === "true");
      window.hidden = !visible;
      count += Number(visible);
    }
    visibleCount.textContent = String(count);
    for (const button of buttons) {
      button.classList.toggle("active", button.dataset.reviewFilter === filter);
    }
  }

  for (const button of buttons) {
    button.addEventListener("click", () => applyFilter(button.dataset.reviewFilter));
  }
})();
