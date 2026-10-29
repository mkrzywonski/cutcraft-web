// Layout override is a browser preference; it never changes the shared project screen.
let layout;
try { layout = localStorage.getItem('cutcraft.layout'); } catch { /* Use viewport default. */ }
const desktop = layout === 'desktop' || (layout !== 'compact' &&
  (innerWidth >= 1024 || (innerWidth >= 768 && matchMedia('(pointer:fine)').matches)));
location.replace(desktop ? '/desktop' : '/station');
