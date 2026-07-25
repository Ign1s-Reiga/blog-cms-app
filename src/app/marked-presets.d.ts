// The package exposes its stylesheet via the "./styles" subpath export, which
// resolves to a plain CSS file. TypeScript can't type a side-effect import of a
// non-".css" specifier, so declare it as a typeless module for the CSS side
// effect (bundled by Next at build time).
declare module "@ign1s-reiga/marked-presets/styles";
