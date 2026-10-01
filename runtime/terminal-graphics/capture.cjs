// Chromium can temporarily lose its capture surface during an offscreen resize.
// Retry only that compositor error, for a bounded interval. Other failures are
// reported immediately and a persistently broken surface remains an error.
async function capture(webContents, delay = ms => new Promise(r => setTimeout(r, ms))) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await webContents.capturePage(undefined, {stayHidden:true, stayAwake:true});
    } catch (error) {
      if (!String(error).includes('UnknownVizError') || attempt === 4) throw error;
      await delay(25 * (attempt + 1));
    }
  }
}
module.exports = {capture};
