import { Component, inject } from '@angular/core';
import { RELOAD } from './reload';

/** Shown when a page couldn't be loaded although the browser is online. */
@Component({
  selector: 'app-unavailable',
  templateUrl: './unavailable.html',
})
export class Unavailable {
  protected readonly retry = inject(RELOAD);
}
