const {contextBridge, ipcRenderer} = require('electron');
contextBridge.exposeInMainWorld('star', {
  scene: callback => ipcRenderer.on('scene', (_, scene) => callback(scene)),
  painted: () => ipcRenderer.send('painted'),
});
