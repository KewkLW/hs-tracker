import { mount } from 'svelte';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import App from './App.svelte';
import Settings from './Settings.svelte';
import Stats from './Stats.svelte';
import Shop from './Shop.svelte';
import Ticker from './Ticker.svelte';

// no default WebView2 context menu anywhere; the overlay draws its own
window.addEventListener('contextmenu', (e) => e.preventDefault());

const label = getCurrentWebviewWindow().label;
const roots = { settings: Settings, stats: Stats, shop: Shop, ticker: Ticker };

export default mount(roots[label] ?? App, {
  target: document.getElementById('app'),
});
