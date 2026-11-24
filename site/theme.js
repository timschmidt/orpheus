// Light / dark / automatic theme switch. Without JavaScript the page
// follows the system setting (palette.css). The choice is remembered in
// localStorage when the browser allows it.
(function () {
  "use strict";
  var KEY = "orpheus-theme";
  var root = document.documentElement;
  function get() {
    try { return localStorage.getItem(KEY) || "auto"; } catch (e) { return "auto"; }
  }
  function apply(t) {
    if (t === "auto") { root.removeAttribute("data-theme"); } else { root.setAttribute("data-theme", t); }
  }
  apply(get());
  document.addEventListener("DOMContentLoaded", function () {
    var b = document.querySelector("button.theme");
    if (!b) { return; }
    var names = { auto: "automatic", light: "light", dark: "dark" };
    var next = { auto: "light", light: "dark", dark: "auto" };
    function label(t) { b.setAttribute("aria-label", "Colour theme: " + names[t] + " (change)"); b.title = "Theme: " + names[t]; }
    label(get());
    b.hidden = false;
    b.addEventListener("click", function () {
      var t = next[get()] || "auto";
      try { localStorage.setItem(KEY, t); } catch (e) { /* private mode: not remembered */ }
      apply(t);
      label(t);
    });
  });
})();
