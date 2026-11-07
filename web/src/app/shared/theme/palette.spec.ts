import styles from '../../../styles.scss' with { loader: 'text' };

// The palettes in `src/styles.scss` meet WCAG 2.1 AA contrast in both themes (THM-07). Every pair
// of colors that is used together is listed here, with the contrast it needs.

type ThemeName = 'light' | 'dark';

/** Text, including links and placeholders (WCAG 1.4.3). */
const TEXT = 4.5;
/** The borders of controls, focus indicators and status graphics (WCAG 1.4.11). */
const GRAPHIC = 3;
/**
 * Two colors drawn together, so that one of them has 3:1 against any background. The background
 * that is hardest for both has the square root of their contrast against each.
 */
const ANY_BACKGROUND = GRAPHIC ** 2;

/** Foreground, background, the contrast needed, and what uses them. Tokens without `--color-`. */
const PAIRS: readonly (readonly [string, string, number, string])[] = [
  ['text', 'bg', TEXT, 'text'],
  ['text', 'surface', TEXT, 'text in the header, buttons, toasts and banners'],
  ['text-muted', 'bg', TEXT, 'secondary text, such as the footer and placeholders'],
  ['text-muted', 'surface', TEXT, 'secondary text on surfaces'],
  ['accent', 'bg', TEXT, 'links, and chosen radio buttons'],
  ['accent', 'surface', TEXT, 'the current page in the navigation'],
  ['on-accent', 'accent', TEXT, 'primary buttons'],
  ['danger', 'bg', TEXT, 'errors'],
  ['danger', 'surface', TEXT, 'errors on surfaces'],
  ['on-danger', 'danger', TEXT, 'buttons for destructive actions'],
  ['success', 'bg', TEXT, 'statuses such as online'],
  ['success', 'surface', TEXT, 'statuses such as online, on surfaces'],
  ['on-success', 'success', TEXT, 'success badges'],
  ['warning', 'bg', TEXT, 'warnings'],
  ['warning', 'surface', TEXT, 'warnings on surfaces'],
  ['on-warning', 'warning', TEXT, 'warning badges'],
  ['border-strong', 'bg', GRAPHIC, 'the borders of controls, such as inputs'],
  ['border-strong', 'surface', GRAPHIC, 'the borders of controls on surfaces'],
  ['focus', 'bg', GRAPHIC, 'the focus indicator'],
  ['focus', 'surface', GRAPHIC, 'the focus indicator on surfaces'],
  ['on-hint', 'hint', TEXT, 'hint labels'],
  ['on-hint', 'hint', ANY_BACKGROUND, 'hint labels and grid lines on any remote screen'],
];

/** Colors that need no contrast, and why. */
const DECORATIVE: Readonly<Record<string, string>> = {
  border: 'separators, and the placeholder rows of a list that is loading',
  backdrop: 'dims the page behind a dialog',
};

interface Palettes {
  /** Every `--color-*` token in `:root`, without `--color-`. */
  names: string[];
  /** The tokens that are opaque colors, by theme. */
  colors: Record<ThemeName, Map<string, string>>;
}

/** Reads the palettes from the `--color-*` tokens in `:root`. */
function readPalettes(css: string): Palettes {
  const root = /^:root \{$([\s\S]*?)^\}$/m.exec(css)?.[1] ?? '';
  const values = new Map<string, string>();
  for (const [, name, value] of root.matchAll(/^\s*--color-([\w-]+):\s*([^;]+);/gm)) {
    values.set(name, value.trim());
  }

  const colors = { light: new Map<string, string>(), dark: new Map<string, string>() };
  const resolve = (name: string, seen: string[] = []): [string, string] | undefined => {
    const value = values.get(name);
    if (value === undefined || seen.includes(name)) {
      return undefined;
    }
    const alias = /^var\(--color-([\w-]+)\)$/.exec(value);
    if (alias) {
      return resolve(alias[1], [...seen, name]);
    }
    const pair = /^light-dark\((#[0-9a-f]+),\s*(#[0-9a-f]+)\)$/i.exec(value);
    if (pair) {
      return [pair[1], pair[2]];
    }
    // The same in both themes.
    return /^#[0-9a-f]+$/i.test(value) ? [value, value] : undefined;
  };
  for (const name of values.keys()) {
    const pair = resolve(name);
    if (pair) {
      colors.light.set(name, pair[0]);
      colors.dark.set(name, pair[1]);
    }
  }
  return { names: [...values.keys()], colors };
}

/** The relative luminance of a `#rgb` or `#rrggbb` color (WCAG 2.1). */
function luminance(color: string): number {
  const hex = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color)?.[1];
  if (!hex) {
    throw new Error(`Can't read ${color}: write colors as #rrggbb`);
  }
  const full = hex.length === 3 ? [...hex].map((digit) => digit + digit).join('') : hex;
  const [r, g, b] = [0, 2, 4].map((start) => {
    const channel = parseInt(full.slice(start, start + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The contrast ratio of two colors (WCAG 2.1), from 1 to 21. Not rounded, as WCAG requires. */
function contrast(a: string, b: string): number {
  const [lighter, darker] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (lighter + 0.05) / (darker + 0.05);
}

describe('Palettes (THM-07)', () => {
  const { names, colors } = readPalettes(styles);

  function color(theme: ThemeName, name: string): string {
    const value = colors[theme].get(name);
    if (!value) {
      throw new Error(`--color-${name} isn't an opaque color in src/styles.scss`);
    }
    return value;
  }

  it('are read from the styles', () => {
    expect(colors.light.get('text')).toMatch(/^#/);
    expect(colors.dark.get('text')).toMatch(/^#/);
    expect(colors.light.get('bg')).not.toBe(colors.dark.get('bg'));
  });

  it('check every color, or say why it needs no contrast', () => {
    const checked = new Set(PAIRS.flatMap(([foreground, background]) => [foreground, background]));

    expect(names.filter((name) => !checked.has(name) && !(name in DECORATIVE))).toEqual([]);
    expect(Object.keys(DECORATIVE).filter((name) => !names.includes(name))).toEqual([]);
  });

  describe.each(['light', 'dark'] as const)('%s', (theme) => {
    it.each(PAIRS)('%s on %s: at least %s:1, for %s', (foreground, background, minimum) => {
      const ratio = contrast(color(theme, foreground), color(theme, background));

      expect(ratio, `${foreground} on ${background}, ${theme}`).toBeGreaterThanOrEqual(minimum);
    });
  });
});
