import { Component, inject } from '@angular/core';
import { Toasts } from './toasts';

/** Shows the toasts of {@link Toasts} at the bottom of the window. */
@Component({
  selector: 'app-toast-region',
  templateUrl: './toast-region.html',
  styleUrl: './toast-region.scss',
})
export class ToastRegion {
  protected readonly toasts = inject(Toasts);
}
