import { TestBed } from '@angular/core/testing';
import { ToastRegion } from './toast-region';
import { TOAST_DURATION_MS, Toasts } from './toasts';

describe('Toasts', () => {
  let toasts: Toasts;

  beforeEach(() => {
    vi.useFakeTimers();
    toasts = TestBed.inject(Toasts);
  });

  afterEach(() => vi.useRealTimers());

  function messages(): string[] {
    return toasts.toasts().map((toast) => toast.message);
  }

  it('disappears after 5 seconds', () => {
    toasts.show('Device paired.');

    vi.advanceTimersByTime(TOAST_DURATION_MS - 1);
    expect(messages()).toEqual(['Device paired.']);
    vi.advanceTimersByTime(1);
    expect(messages()).toEqual([]);
  });

  it('pauses while held and then runs for the time that was left', () => {
    toasts.show('Transfer complete.');
    const { id } = toasts.toasts()[0];
    vi.advanceTimersByTime(2000);

    toasts.hold(id, 'hover');
    vi.advanceTimersByTime(60_000);
    expect(messages()).toEqual(['Transfer complete.']);

    toasts.release(id, 'hover');
    vi.advanceTimersByTime(TOAST_DURATION_MS - 2000 - 1);
    expect(messages()).toEqual(['Transfer complete.']);
    vi.advanceTimersByTime(1);
    expect(messages()).toEqual([]);
  });

  it('stays paused until neither hover nor focus holds it', () => {
    toasts.show('Device paired.');
    const { id } = toasts.toasts()[0];

    toasts.hold(id, 'hover');
    toasts.hold(id, 'focus');
    toasts.release(id, 'hover');
    vi.advanceTimersByTime(60_000);
    expect(messages()).toEqual(['Device paired.']);

    toasts.release(id, 'focus');
    vi.advanceTimersByTime(TOAST_DURATION_MS);
    expect(messages()).toEqual([]);
  });

  it('can be dismissed', () => {
    toasts.show('First.');
    toasts.show('Second.');

    toasts.dismiss(toasts.toasts()[0].id);

    expect(messages()).toEqual(['Second.']);
  });
});

describe('ToastRegion', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  // detectChanges(), because whenStable() waits for timers, which are fake here.
  it('shows toasts in a live region that pauses them on hover and focus', () => {
    const fixture = TestBed.createComponent(ToastRegion);
    fixture.detectChanges();
    const region: HTMLElement = fixture.nativeElement.querySelector('[role="status"]');
    expect(region).toBeTruthy();

    TestBed.inject(Toasts).show('Device paired.');
    fixture.detectChanges();
    const toast = region.querySelector<HTMLElement>('.toast')!;
    expect(toast.textContent).toContain('Device paired.');

    toast.dispatchEvent(new MouseEvent('mouseenter'));
    toast.querySelector('button')!.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
    toast.dispatchEvent(new MouseEvent('mouseleave'));
    vi.advanceTimersByTime(60_000);
    fixture.detectChanges();
    expect(region.textContent).toContain('Device paired.');

    toast.querySelector('button')!.click();
    fixture.detectChanges();
    expect(region.querySelector('.toast')).toBeNull();
  });
});
