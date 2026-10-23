(() => {
  const r = id => { const b = document.getElementById(id).getBoundingClientRect(); return [Math.round(b.x), Math.round(b.y + scrollY), Math.round(b.width), Math.round(b.height)]; };
  const fs = sel => getComputedStyle(document.querySelector(sel)).fontSize;
  const mq = ['(max-width: 767px)','(min-width: 640px)','(min-width: 1024px)','(min-width: 1536px)','(orientation: portrait)','(hover: hover)','(pointer: fine)','(pointer: coarse)','(any-pointer: coarse)','(prefers-color-scheme: dark)','(prefers-reduced-motion: reduce)','(min-resolution: 2dppx)','(min-resolution: 3dppx)','(display-mode: browser)'];
  const out = {
    inner: [innerWidth, innerHeight], client: [document.documentElement.clientWidth, document.documentElement.clientHeight],
    outer: [outerWidth, outerHeight], screen: [screen.width, screen.height, screen.availWidth, screen.availHeight],
    dpr: devicePixelRatio, visual: visualViewport ? [visualViewport.width, visualViewport.height, visualViewport.scale] : null,
    scrollHeight: document.documentElement.scrollHeight,
    fonts: { body: fs('.card'), h3: fs('.card h3'), small: fs('.small'), tiny: fs('.tiny'), hero: fs('.hero'), nav: fs('nav a'), foot: fs('footer'), p: fs('.long p') },
    rects: { hero: r('hero'), c1: r('c1'), c2: r('c2'), c4: r('c4'), cq: r('cq'), vw: r('vw'), foot: r('foot') },
    navShown: getComputedStyle(document.querySelector('nav')).display, cqBg: getComputedStyle(document.getElementById('cq')).backgroundColor,
    img: document.getElementById('img').currentSrc.includes('360') ? '3x' : document.getElementById('img').currentSrc.includes('240') ? '2x' : '1x',
    mq: Object.fromEntries(mq.map(q => [q, matchMedia(q).matches])),
    touch: [navigator.maxTouchPoints, 'ontouchstart' in window], ua: navigator.userAgent,
  };
  return JSON.stringify(out);
})()
