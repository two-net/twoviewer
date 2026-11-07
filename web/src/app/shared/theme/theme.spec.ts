import { TestBed } from '@angular/core/testing';
import bootScript from '../../../../public/theme.js' with { loader: 'text' };
import { REDUCED_MOTION } from '../motion/reduced-motion';
import { THEME_STORAGE_KEY, Theme } from './theme';

const root = document.documentElement;

function cleanUp(): void {
  localStorage.clear();
  root.removeAttribute('data-theme');
  root.classList.remove('theme-change');
  Reflect.deleteProperty(document, 'startViewTransition');
  vi.restoreAllMocks();
}

/** jsdom has no view transitions. These run the update at once and finish when told to. */
function fakeViewTransitions() {
  const finishers: (() => void)[] = [];
  const start = vi.fn((update: () => void) => {
    update();
    const finished = new Promise<void>((resolve) => finishers.push(resolve));
    return { finished } as ViewTransition;
  });
  Object.defineProperty(document, 'startViewTransition', { configurable: true, value: start });
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  return {
    start,
    /** Ends the `index`th cross-fade, and lets its promise callbacks run. */
    async finish(index: number): Promise<void> {
      finishers[index]();
      await Promise.resolve();
      await Promise.resolve();
    },
  };
}

describe('Theme', () => {
  let reducedMotion: boolean;

  beforeEach(() => {
    reducedMotion = false;
    TestBed.configureTestingModule({
      providers: [{ provide: REDUCED_MOTION, useValue: () => reducedMotion }],
    });
  });

  afterEach(cleanUp);

  it('is System by default, which leaves the colors to the system (THM-01)', () => {
    const theme = TestBed.inject(Theme);

    expect(theme.option()).toBe('system');
    expect(root.hasAttribute('data-theme')).toBe(false);
  });

  it('starts with the option remembered in this browser (THM-04)', () => {
    localStorage.setItem(THEME_STORAGE_KEY, 'dark');

    const theme = TestBed.inject(Theme);

    expect(theme.option()).toBe('dark');
    expect(root.getAttribute('data-theme')).toBe('dark');
  });

  it('treats an unknown remembered value as System', () => {
    localStorage.setItem(THEME_STORAGE_KEY, 'sepia');

    expect(TestBed.inject(Theme).option()).toBe('system');
  });

  it('applies a chosen option and remembers it', () => {
    const theme = TestBed.inject(Theme);

    theme.choose('dark');
    expect(theme.option()).toBe('dark');
    expect(root.getAttribute('data-theme')).toBe('dark');
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe('dark');

    theme.choose('light');
    expect(root.getAttribute('data-theme')).toBe('light');

    theme.choose('system');
    expect(root.hasAttribute('data-theme')).toBe(false);
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe('system');
  });

  it('cross-fades a change (THM-03)', async () => {
    const transitions = fakeViewTransitions();
    const theme = TestBed.inject(Theme);

    theme.choose('dark');

    expect(transitions.start).toHaveBeenCalledTimes(1);
    expect(root.getAttribute('data-theme')).toBe('dark');
    expect(root.classList).toContain('theme-change');
    await transitions.finish(0);
    expect(root.classList).not.toContain('theme-change');
  });

  it('keeps the cross-fade style for a change that skips the previous one', async () => {
    const transitions = fakeViewTransitions();
    const theme = TestBed.inject(Theme);

    theme.choose('dark');
    theme.choose('light');
    await transitions.finish(0);

    expect(root.classList).toContain('theme-change');
    await transitions.finish(1);
    expect(root.classList).not.toContain('theme-change');
  });

  it('changes at once with reduced motion', () => {
    const transitions = fakeViewTransitions();
    reducedMotion = true;

    TestBed.inject(Theme).choose('dark');

    expect(transitions.start).not.toHaveBeenCalled();
    expect(root.getAttribute('data-theme')).toBe('dark');
    expect(root.classList).not.toContain('theme-change');
  });

  it('changes at once in browsers without view transitions', () => {
    TestBed.inject(Theme).choose('dark');

    expect(root.getAttribute('data-theme')).toBe('dark');
  });

  it('does not cross-fade when the option stays the same', () => {
    const transitions = fakeViewTransitions();

    TestBed.inject(Theme).choose('system');

    expect(transitions.start).not.toHaveBeenCalled();
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe('system');
  });

  it('works when the browser blocks storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new DOMException('Blocked', 'SecurityError');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('Blocked', 'SecurityError');
    });
    const theme = TestBed.inject(Theme);
    expect(theme.option()).toBe('system');

    theme.choose('dark');

    expect(theme.option()).toBe('dark');
    expect(root.getAttribute('data-theme')).toBe('dark');
  });

  it('follows a change made in another tab', () => {
    const theme = TestBed.inject(Theme);

    window.dispatchEvent(new StorageEvent('storage', { key: THEME_STORAGE_KEY, newValue: 'dark' }));
    expect(theme.option()).toBe('dark');
    expect(root.getAttribute('data-theme')).toBe('dark');

    window.dispatchEvent(new StorageEvent('storage', { key: THEME_STORAGE_KEY, newValue: null }));
    expect(theme.option()).toBe('system');
    expect(root.hasAttribute('data-theme')).toBe(false);
  });

  it('ignores other keys in storage', () => {
    const theme = TestBed.inject(Theme);
    theme.choose('light');

    window.dispatchEvent(new StorageEvent('storage', { key: 'other', newValue: 'dark' }));

    expect(theme.option()).toBe('light');
  });
});

// public/theme.js, which applies the remembered option before the first paint (THM-05).
describe('theme.js', () => {
  afterEach(cleanUp);

  function runBootScript(): void {
    new Function(bootScript)();
  }

  it.each(['light', 'dark'])('applies a remembered %s', (option) => {
    localStorage.setItem(THEME_STORAGE_KEY, option);

    runBootScript();

    expect(root.getAttribute('data-theme')).toBe(option);
  });

  it.each(['system', 'sepia', null])('leaves %s to the system', (option) => {
    if (option !== null) {
      localStorage.setItem(THEME_STORAGE_KEY, option);
    }

    runBootScript();

    expect(root.hasAttribute('data-theme')).toBe(false);
  });

  it('leaves the system theme when the browser blocks storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new DOMException('Blocked', 'SecurityError');
    });

    expect(runBootScript).not.toThrow();
    expect(root.hasAttribute('data-theme')).toBe(false);
  });

  it('reads what Theme remembers', () => {
    TestBed.inject(Theme).choose('dark');
    root.removeAttribute('data-theme');

    runBootScript();

    expect(root.getAttribute('data-theme')).toBe('dark');
  });
});
