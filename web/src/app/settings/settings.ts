import { Component } from '@angular/core';
import { MotionPicker } from '../shared/motion/motion-picker';
import { ThemePicker } from '../shared/theme/theme-picker';
import { SignIns } from './sign-ins';

@Component({
  selector: 'app-settings',
  imports: [MotionPicker, SignIns, ThemePicker],
  templateUrl: './settings.html',
  styleUrl: './settings.scss',
})
export class Settings {}
