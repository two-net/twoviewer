import { ComponentFixture, TestBed } from '@angular/core/testing';
import { ConfirmDialog } from './confirm-dialog';
import { Confirmation, matchesTyped } from './confirmation';

// jsdom has no modal dialogs.
function fakeDialogs(): void {
  Object.defineProperty(HTMLDialogElement.prototype, 'open', {
    configurable: true,
    get(this: HTMLDialogElement) {
      return this.hasAttribute('open');
    },
  });
  HTMLDialogElement.prototype.showModal = function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  };
  HTMLDialogElement.prototype.close = function (this: HTMLDialogElement) {
    if (this.hasAttribute('open')) {
      this.removeAttribute('open');
      this.dispatchEvent(new Event('close'));
    }
  };
}

describe('ConfirmDialog', () => {
  let fixture: ComponentFixture<ConfirmDialog>;
  let confirmation: Confirmation;
  let dialog: HTMLDialogElement;

  beforeAll(fakeDialogs);

  beforeEach(async () => {
    fixture = TestBed.createComponent(ConfirmDialog);
    confirmation = TestBed.inject(Confirmation);
    dialog = fixture.nativeElement.querySelector('dialog');
    await fixture.whenStable();
  });

  // Wrapped, because an async function returning the answer would wait for it.
  async function ask(typeToConfirm?: string): Promise<{ answer: Promise<boolean> }> {
    const answer = confirmation.confirm({
      title: 'Remove device?',
      message: 'Its sessions end.',
      confirmLabel: 'Remove device',
      typeToConfirm,
    });
    await fixture.whenStable();
    return { answer };
  }

  function button(label: string): HTMLButtonElement {
    return [...dialog.querySelectorAll('button')].find(
      (button) => button.textContent?.trim() === label,
    )!;
  }

  async function type(text: string): Promise<void> {
    const input = dialog.querySelector('input')!;
    input.value = text;
    input.dispatchEvent(new Event('input'));
    await fixture.whenStable();
  }

  it('shows the question and answers true on confirm', async () => {
    const { answer } = await ask();
    expect(dialog.open).toBe(true);
    expect(dialog.textContent).toContain('Remove device?');
    expect(dialog.textContent).toContain('Its sessions end.');

    button('Remove device').click();
    await fixture.whenStable();

    expect(await answer).toBe(true);
    expect(dialog.open).toBe(false);
  });

  it('answers false on Cancel', async () => {
    const { answer } = await ask();

    button('Cancel').click();
    await fixture.whenStable();

    expect(await answer).toBe(false);
    expect(dialog.open).toBe(false);
  });

  it('answers false when the dialog closes, as on Escape', async () => {
    const { answer } = await ask();

    dialog.close();

    expect(await answer).toBe(false);
  });

  it('answers false when a new question replaces it', async () => {
    const { answer: first } = await ask();

    await ask();

    expect(await first).toBe(false);
  });

  it('confirms only once the expected text is typed', async () => {
    const { answer } = await ask('ann.lee@example.com');
    const confirm = button('Remove device');
    expect(dialog.textContent).toContain('ann.lee@example.com');
    expect(confirm.disabled).toBe(true);

    await type('ann.lee@example.co');
    expect(confirm.disabled).toBe(true);
    confirm.closest('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
    await fixture.whenStable();
    expect(dialog.open).toBe(true);

    await type(' Ann.Lee@Example.com ');
    expect(confirm.disabled).toBe(false);
    confirm.click();
    await fixture.whenStable();
    expect(await answer).toBe(true);
  });

  it('starts every question with an empty text field', async () => {
    await ask('ann.lee@example.com');
    await type('ann.lee@example.com');
    button('Cancel').click();
    await fixture.whenStable();

    await ask('ann.lee@example.com');

    expect(dialog.querySelector('input')!.value).toBe('');
    expect(button('Remove device').disabled).toBe(true);
  });
});

describe('matchesTyped', () => {
  it('ignores case and surrounding spaces', () => {
    expect(matchesTyped('  Ann.Lee@Example.COM ', 'ann.lee@example.com')).toBe(true);
    expect(matchesTyped('ann.lee@example', 'ann.lee@example.com')).toBe(false);
    expect(matchesTyped('ann .lee@example.com', 'ann.lee@example.com')).toBe(false);
  });
});
