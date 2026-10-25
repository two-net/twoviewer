import { Component, computed, input } from '@angular/core';
import { describeError } from './user-error';

/** Shows an error as {@link describeError} describes it, announced to screen readers. */
@Component({
  selector: 'app-error-message',
  templateUrl: './error-message.html',
  styleUrl: './error-message.scss',
})
export class ErrorMessage {
  readonly error = input.required<unknown>();
  protected readonly message = computed(() => describeError(this.error()));
}
