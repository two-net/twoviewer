import { TestBed } from '@angular/core/testing';
import bootScript from '../../../../public/appearance.js' with { loader: 'text' };
import { MOTION_STORAGE_KEY, Motion } from './motion';
import { REDUCED_MOTION } from './reduced-motion';

const root = document.documentElement;

function cleanUp(): void {
  localStorage.clear();
  root.removeAttribute('data-motion');
  Reflect.deleteProperty(window, 'matchMedia');
  vi.restoreAllMocks();
}

/** jsdom has no `matchMedia`. This one answers the reduced-motion query, and can change. */
function fakeSystemPreference(reduce: boolean) {
  const listeners = new Set<() => void>();
  const query = {
    matches: reduce,
    addEventListener: (_type: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_type: string, listener: () => void) => listeners.delete(listener),
  };
  const matchMedia = vi.fn(() => query);
  Object.defineProperty(window, 'matchMedia', { configurable: true, value: matchMedia });
  return {
    matchMedia,
    listeners,
    /** Changes the preference, as the user does in the operating system's settings. */
    set(value: boolean): void {
      query.matches = value;
      listeners.forEach((listener) => listener());
    },
  };
}

describe('Motion', () => {
  afterEach(cleanUp);

  it('is System by default, which leaves motion to the system (ANI-05)', () => {
    const motion = TestBed.inject(Motion);

    expect(motion.option()).toBe('system');
    expect(root.hasAttribute('data-motion')).toBe(false);
    expect(motion.reduced()).toBe(false);
  });

  it('with System, reduces motion while the system asks for it (ANI-04)', () => {
    const system = fakeSystemPreference(true);
    const motion = TestBed.inject(Motion);

    expect(system.matchMedia).toHaveBeenCalledWith('(prefers-reduced-motion: reduce)');
    expect(motion.reduced()).toBe(true);

    system.set(false);
    expect(motion.reduced()).toBe(false);

    system.set(true);
    expect(motion.reduced()).toBe(true);
  });

  it('with Full or Reduced, ignores what the system asks for', () => {
    const system = fakeSystemPreference(true);
    const motion = TestBed.inject(Motion);

    motion.choose('full');
    expect(motion.reduced()).toBe(false);

    system.set(false);
    motion.choose('reduced');
    expect(motion.reduced()).toBe(true);
  });

  it('applies a chosen option and remembers it', () => {
    const motion = TestBed.inject(Motion);

    motion.choose('reduced');
    expect(motion.option()).toBe('reduced');
    expect(root.getAttribute('data-motion')).toBe('reduced');
    expect(localStorage.getItem(MOTION_STORAGE_KEY)).toBe('reduced');

    motion.choose('full');
    expect(root.getAttribute('data-motion')).toBe('full');

    motion.choose('system');
    expect(root.hasAttribute('data-motion')).toBe(false);
    expect(localStorage.getItem(MOTION_STORAGE_KEY)).toBe('system');
  });

  it('starts with the option remembered in this browser', () => {
    localStorage.setItem(MOTION_STORAGE_KEY, 'reduced');

    const motion = TestBed.inject(Motion);

    expect(motion.option()).toBe('reduced');
    expect(motion.reduced()).toBe(true);
    expect(root.getAttribute('data-motion')).toBe('reduced');
  });

  it('treats an unknown remembered value as System', () => {
    localStorage.setItem(MOTION_STORAGE_KEY, 'fast');

    expect(TestBed.inject(Motion).option()).toBe('system');
  });

  it('works when the browser blocks storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new DOMException('Blocked', 'SecurityError');
    });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException('Blocked', 'SecurityError');
    });
    const motion = TestBed.inject(Motion);
    expect(motion.option()).toBe('system');

    motion.choose('reduced');

    expect(motion.reduced()).toBe(true);
    expect(root.getAttribute('data-motion')).toBe('reduced');
  });

  it('follows a change made in another tab', () => {
    const motion = TestBed.inject(Motion);

    window.dispatchEvent(
      new StorageEvent('storage', { key: MOTION_STORAGE_KEY, newValue: 'reduced' }),
    );
    expect(motion.option()).toBe('reduced');
    expect(root.getAttribute('data-motion')).toBe('reduced');

    window.dispatchEvent(new StorageEvent('storage', { key: MOTION_STORAGE_KEY, newValue: null }));
    expect(motion.option()).toBe('system');
    expect(root.hasAttribute('data-motion')).toBe(false);
  });

  it('ignores other keys in storage', () => {
    const motion = TestBed.inject(Motion);
    motion.choose('full');

    window.dispatchEvent(new StorageEvent('storage', { key: 'other', newValue: 'reduced' }));

    expect(motion.option()).toBe('full');
  });

  it('stops listening to the system when the app is destroyed', () => {
    const system = fakeSystemPreference(false);
    TestBed.inject(Motion);
    expect(system.listeners.size).toBe(1);

    TestBed.resetTestingModule();

    expect(system.listeners.size).toBe(0);
  });
});

describe('REDUCED_MOTION', () => {
  afterEach(cleanUp);

  it('follows the motion option, read at each call', () => {
    const reduced = TestBed.inject(REDUCED_MOTION);
    expect(reduced()).toBe(false);

    TestBed.inject(Motion).choose('reduced');

    expect(reduced()).toBe(true);
  });

  it('follows the system with System', () => {
    const system = fakeSystemPreference(true);
    const reduced = TestBed.inject(REDUCED_MOTION);
    expect(reduced()).toBe(true);

    system.set(false);

    expect(reduced()).toBe(false);
  });
});

// public/appearance.js, which applies the remembered option before the first paint (SPL-07).
describe('appearance.js: motion', () => {
  afterEach(cleanUp);

  function runBootScript(): void {
    new Function(bootScript)();
  }

  it.each(['full', 'reduced'])('applies a remembered %s', (option) => {
    localStorage.setItem(MOTION_STORAGE_KEY, option);

    runBootScript();

    expect(root.getAttribute('data-motion')).toBe(option);
  });

  it.each(['system', 'fast', null])('leaves %s to the system', (option) => {
    if (option !== null) {
      localStorage.setItem(MOTION_STORAGE_KEY, option);
    }

    runBootScript();

    expect(root.hasAttribute('data-motion')).toBe(false);
  });

  it('leaves motion to the system when the browser blocks storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new DOMException('Blocked', 'SecurityError');
    });

    expect(runBootScript).not.toThrow();
    expect(root.hasAttribute('data-motion')).toBe(false);
  });

  it('reads what Motion remembers', () => {
    TestBed.inject(Motion).choose('reduced');
    root.removeAttribute('data-motion');

    runBootScript();

    expect(root.getAttribute('data-motion')).toBe('reduced');
  });
});
