// Specs import `public/theme.js` as text (`with { loader: 'text' }`) to run it.
declare module '*/public/theme.js' {
  const text: string;
  export default text;
}
