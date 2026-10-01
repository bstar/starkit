// Trusted local renderer. Application data is never loaded as web content.
const {app, BrowserWindow, ipcMain} = require('electron');
const path = require('node:path');
app.setName('STAR/KIT graphical renderer');
app.setPath('userData', path.join(__dirname, 'profile'));
if (process.env.STAR_GRAPHICS_SOFTWARE === '1') app.disableHardwareAcceleration();
app.commandLine.appendSwitch('disable-background-timer-throttling');
app.commandLine.appendSwitch('force-device-scale-factor', '1');
app.commandLine.appendSwitch('disable-renderer-backgrounding');
let window, pending, rendering = false, closing = false;
let ready;
const emit = value => process.stdout.write(JSON.stringify(value) + '\n');
async function render() {
  if (!window || rendering || !pending || closing) return;
  rendering = true;
  const scene = pending; pending = undefined;
  try {
    const {width, height} = scene.viewport;
    if (width > 8192 || height > 8192 || width * height > 32000000) throw Error('Viewport too large');
    window.setContentSize(width, height);
    let timeout;
    const done = new Promise((resolve,reject) => {
      ready = resolve;
      timeout = setTimeout(() => reject(Error('Scene paint timed out')), 5000);
    });
    window.webContents.send('scene', scene);
    try { await done; } finally { clearTimeout(timeout); }
    const image = await window.webContents.capturePage(undefined, {stayHidden:true, stayAwake:true});
    const png = image.toPNG();
    if (png.length > 11000000) throw Error('Rendered frame exceeds transport limit');
    const size = image.getSize();
    emit({type:'frame', revision:scene.revision, generation:scene.viewport.generation,
      width:size.width, height:size.height, png:png.toString('base64')});
  } catch (error) { emit({type:'error', message:String(error)}); }
  finally { ready = undefined; rendering = false; if (pending) setImmediate(render); }
}
ipcMain.on('painted', () => ready?.());
app.whenReady().then(async () => {
  window = new BrowserWindow({width:1200,height:800,useContentSize:true,show:false,
    frame:false,paintWhenInitiallyHidden:true,skipTaskbar:true,
    webPreferences:{offscreen:true,contextIsolation:true,nodeIntegration:false,sandbox:true,
      backgroundThrottling:false,preload:path.join(__dirname,'preload.cjs')}});
  window.webContents.setFrameRate(30);
  window.webContents.setWindowOpenHandler(() => ({action:'deny'}));
  window.webContents.on('will-navigate', event => event.preventDefault());
  window.webContents.on('render-process-gone', (_, details) => {
    emit({type:'error', message:`Renderer exited: ${details.reason}`}); app.exit(1);
  });
  await window.loadFile(path.join(__dirname,'index.html'));
  emit({type:'ready'}); render();
});
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => {
  input += chunk;
  if (Buffer.byteLength(input) > 16777216) {emit({type:'error',message:'Input exceeds limit'});app.exit(1);return;}
  let newline;
  while ((newline = input.indexOf('\n')) >= 0) {
    const line = input.slice(0,newline); input = input.slice(newline+1);
    try {
      const message = JSON.parse(line);
      if (message.type === 'scene') {pending = message.scene; render();}
      else if (message.type === 'clipboard') require('electron').clipboard.writeText(message.text);
      else if (message.type === 'close') {closing = true; app.quit();}
    } catch (error) {emit({type:'error',message:String(error)});}
  }
});
process.stdin.on('end', () => {closing = true; app.quit();});
app.on('window-all-closed', () => app.quit());
process.on('SIGTERM', () => app.exit());
