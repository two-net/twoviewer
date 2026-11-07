// Applies the appearance options remembered in this browser before the first paint, so that no
// frame, including the splash screen, shows the other theme (THM-04, THM-05) or more motion than
// the user asked for (SPL-07, ANI-05). `index.html` loads it in `<head>` without `async` or `defer`,
// so it runs before the page is drawn. It is a file rather than an inline script because the
// Content-Security-Policy blocks inline scripts.
//
// `Theme` in `src/app/shared/theme/theme.ts` and `Motion` in `src/app/shared/motion/motion.ts` use
// the same storage keys and attributes. Without a stored option, nothing is set and the styles
// follow the system (THM-01, THM-02, ANI-05).
(function () {
  var root = document.documentElement;
  try {
    var theme = localStorage.getItem('twoviewer.theme');
    if (theme === 'light' || theme === 'dark') {
      root.dataset.theme = theme;
    }
    var motion = localStorage.getItem('twoviewer.motion');
    if (motion === 'full' || motion === 'reduced') {
      root.dataset.motion = motion;
    }
  } catch (error) {
    // Storage is blocked: System applies.
  }
})();
