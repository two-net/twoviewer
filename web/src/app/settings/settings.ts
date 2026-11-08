import { Component } from '@angular/core';
import { MotionPicker } from '../shared/motion/motion-picker';
import { ThemePicker } from '../shared/theme/theme-picker';
import { AccountSettings } from './account-settings';
import { MfaSettings } from './mfa-settings';
import { PasswordSettings } from './password-settings';
import { SignIns } from './sign-ins';

@Component({
  selector: 'app-settings',
  imports: [AccountSettings, MfaSettings, MotionPicker, PasswordSettings, SignIns, ThemePicker],
  templateUrl: './settings.html',
  styleUrl: './settings.scss',
})
export class Settings {}
