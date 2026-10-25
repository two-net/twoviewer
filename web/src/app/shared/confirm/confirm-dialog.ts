import {
  Component,
  ElementRef,
  afterRenderEffect,
  computed,
  inject,
  linkedSignal,
  viewChild,
} from '@angular/core';
import { Confirmation, matchesTyped } from './confirmation';

/**
 * Shows the open question of {@link Confirmation} in a modal dialog. The native `<dialog>` keeps
 * focus inside, makes the rest of the page inert and cancels on Escape.
 */
@Component({
  selector: 'app-confirm-dialog',
  templateUrl: './confirm-dialog.html',
  styleUrl: './confirm-dialog.scss',
})
export class ConfirmDialog {
  protected readonly confirmation = inject(Confirmation);
  /** What the user typed for `typeToConfirm`. Empty again for every new question. */
  protected readonly typed = linkedSignal({
    source: this.confirmation.request,
    computation: () => '',
  });
  protected readonly canConfirm = computed(() => {
    const expected = this.confirmation.request()?.typeToConfirm;
    return expected === undefined || matchesTyped(this.typed(), expected);
  });
  private readonly dialog = viewChild.required<ElementRef<HTMLDialogElement>>('dialog');

  constructor() {
    // After rendering, so that showModal() finds the question's controls to focus.
    afterRenderEffect(() => {
      const request = this.confirmation.request();
      const dialog = this.dialog().nativeElement;
      if (request && !dialog.open) {
        dialog.showModal();
      } else if (!request && dialog.open) {
        dialog.close();
      }
    });
  }

  protected submit(event: Event): void {
    event.preventDefault();
    if (this.canConfirm()) {
      this.confirmation.answer(true);
    }
  }
}
