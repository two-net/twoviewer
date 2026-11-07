import { Component, inject } from '@angular/core';
import { Motion } from './motion';

let pickers = 0;

/** The motion option as a group of radio buttons, in Settings → Appearance (ANI-05). */
@Component({
  selector: 'app-motion-picker',
  templateUrl: './motion-picker.html',
})
export class MotionPicker {
  protected readonly motion = inject(Motion);
  /** Each picker is its own radio group, in case two are on the page at once. */
  protected readonly name = `motion-${++pickers}`;
}
