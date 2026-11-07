import { Component } from '@angular/core';
import { MotionPicker } from '../shared/motion/motion-picker';
import { ThemePicker } from '../shared/theme/theme-picker';
import { MfaSettings } from './mfa-settings';
import { SignIns } from './sign-ins';

@Component({
  selector: 'app-settings',
  imports: [MfaSettings, MotionPicker, SignIns, ThemePicker],
  templateUrl: './settings.html',
  styleUrl: './settings.scss',
})
export class Settings {}
