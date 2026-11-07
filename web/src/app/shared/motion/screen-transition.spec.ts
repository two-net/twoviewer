import { TestBed } from '@angular/core/testing';
import { ActivatedRouteSnapshot } from '@angular/router';
import { REDUCED_MOTION } from './reduced-motion';
import { screenTransition } from './screen-transition';

const root = document.documentElement;

/** A view transition that finishes when told to. jsdom has none. */
function fakeTransition() {
  let resolve!: () => void;
  const finished = new Promise<void>((done) => (resolve = done));
  const transition = { finished, skipTransition: vi.fn() } as unknown as ViewTransition;
  return {
    transition,
    /** Ends the transition, and lets its promise callbacks run. */
    async finish(): Promise<void> {
      resolve();
      await Promise.resolve();
      await Promise.resolve();
    },
  };
}

describe('screenTransition', () => {
  let reducedMotion: boolean;

  beforeEach(() => {
    reducedMotion = false;
    TestBed.configureTestingModule({
      providers: [{ provide: REDUCED_MOTION, useValue: () => reducedMotion }],
    });
  });

  afterEach(() => root.classList.remove('screen-change'));

  /** What the router does once it has started the view transition for a new screen. */
  function created(transition: ViewTransition): void {
    const route = {} as ActivatedRouteSnapshot;
    TestBed.runInInjectionContext(() => screenTransition({ transition, from: route, to: route }));
  }

  it('cross-fades the new screen (ANI-01)', async () => {
    const fake = fakeTransition();

    created(fake.transition);

    expect(root.classList).toContain('screen-change');
    await fake.finish();
    expect(root.classList).not.toContain('screen-change');
  });

  it('keeps the cross-fade style for a change that skips the previous one', async () => {
    const first = fakeTransition();
    const second = fakeTransition();

    created(first.transition);
    created(second.transition);
    await first.finish();

    expect(root.classList).toContain('screen-change');
    await second.finish();
    expect(root.classList).not.toContain('screen-change');
  });

  it('does not cross-fade with reduced motion (ANI-04)', () => {
    reducedMotion = true;
    const fake = fakeTransition();

    created(fake.transition);

    expect(root.classList).not.toContain('screen-change');
    // Skipping would make the router report an error; without the class it has no animation.
    expect(fake.transition.skipTransition).not.toHaveBeenCalled();
  });
});
