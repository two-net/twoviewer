import { Component } from '@angular/core';
import { MotionPicker } from '../shared/motion/motion-picker';
import { ThemePicker } from '../shared/theme/theme-picker';

@Component({
  selector: 'app-settings',
  imports: [MotionPicker, ThemePicker],
  templateUrl: './settings.html',
  styleUrl: './settings.scss',
})
export class Settings {}
