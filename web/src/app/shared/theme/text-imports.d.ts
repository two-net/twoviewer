// Specs import some files as text (`with { loader: 'text' }`): `public/theme.js`, to run it, and
// `src/styles.scss`, to check the palettes' contrast.
declare module '*/public/theme.js' {
  const text: string;
  export default text;
}

declare module '*/styles.scss' {
  const text: string;
  export default text;
}
