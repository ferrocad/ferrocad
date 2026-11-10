/*
 * Progressive-enhancement tabs for the UI-control figures.
 *
 * Without this script both panels remain visible, so the source and a plain
 * GitHub preview still show every variant. In mdBook the first tab is selected
 * on load and arrow keys move between tabs.
 */
(function () {
  "use strict";

  function setup(root) {
    var tabs = Array.prototype.slice.call(
      root.querySelectorAll("[data-fc-tab]")
    );
    var panels = Array.prototype.slice.call(
      root.querySelectorAll("[data-fc-panel]")
    );
    if (!tabs.length || !panels.length) {
      return;
    }

    function select(name) {
      tabs.forEach(function (tab) {
        var active = tab.dataset.fcTab === name;
        tab.classList.toggle("is-active", active);
        tab.setAttribute("aria-selected", active ? "true" : "false");
        tab.setAttribute("tabindex", active ? "0" : "-1");
      });
      panels.forEach(function (panel) {
        panel.hidden = panel.dataset.fcPanel !== name;
      });
    }

    tabs.forEach(function (tab, index) {
      tab.addEventListener("click", function () {
        select(tab.dataset.fcTab);
      });
      tab.addEventListener("keydown", function (event) {
        if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") {
          return;
        }
        event.preventDefault();
        var offset = event.key === "ArrowRight" ? 1 : -1;
        var next = (index + offset + tabs.length) % tabs.length;
        tabs[next].focus();
        select(tabs[next].dataset.fcTab);
      });
    });

    // The markup marks the first tab selected; apply it so the panels agree
    // even when the HTML was authored with all panels visible.
    select(tabs[0].dataset.fcTab);
  }

  function init() {
    document.querySelectorAll("[data-fc-tabs]").forEach(setup);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
