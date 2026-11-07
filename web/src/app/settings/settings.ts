import { Component } from '@angular/core';
import { ThemePicker } from '../shared/theme/theme-picker';

@Component({
  selector: 'app-settings',
  imports: [ThemePicker],
  templateUrl: './settings.html',
})
export class Settings {}
