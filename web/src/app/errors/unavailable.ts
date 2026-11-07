import { Component, inject } from '@angular/core';
import { RELOAD } from './reload';
import { PRODUCT_NAME } from '../product';

/** Shown when a page couldn't be loaded although the browser is online. */
@Component({
  selector: 'app-unavailable',
  templateUrl: './unavailable.html',
})
export class Unavailable {
  protected readonly productName = PRODUCT_NAME;
  protected readonly retry = inject(RELOAD);
}
