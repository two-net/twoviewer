// Applies the theme option remembered in this browser before the first paint (THM-04, THM-05), so
// that no frame, including the splash screen, shows the other theme. `index.html` loads it in
// `<head>` without `async` or `defer`, so it runs before the page is drawn. It is a file rather
// than an inline script because the Content-Security-Policy blocks inline scripts.
//
// `Theme` in `src/app/shared/theme/theme.ts` uses the same storage key and attribute. Without a
// stored Light or Dark, nothing is set and the styles follow the system (THM-01, THM-02).
(function () {
  try {
    var option = localStorage.getItem('twoviewer.theme');
    if (option === 'light' || option === 'dark') {
      document.documentElement.dataset.theme = option;
    }
  } catch (error) {
    // Storage is blocked: System applies.
  }
})();
