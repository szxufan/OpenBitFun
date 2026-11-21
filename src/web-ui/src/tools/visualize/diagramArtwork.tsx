// Fixed vector artwork for illustrated flows. Colors come from the figure theme.
export const diagramArtwork = {
  desktop: (
    <>
      <g transform="matrix(1 .21 -.44 .72 38 6)">
        <path className="visual-guide__side" d="M9 65h96l7 5H16Z M105 0l7 5v65l-7-5Z"/>
        <path className="visual-guide__face" d="M9 0h96v65H9Z"/>
        <path className="visual-guide__side" d="M16 8h82v10H16Z"/>
        <path className="visual-guide__inset" d="M16 18h82v39H16Z M38 18v39 M21 25h11m-11 7h8m-8 7h11 M45 28h35m-35 7h22m-22 7h36"/>
        <path className="visual-guide__accent" d="M45 48h18v4H45Z"/>
      </g>
      <g transform="matrix(1 .21 -.44 .72 57 66)">
        <path className="visual-guide__side" d="M0 0h70v9H0Z"/>
        <path className="visual-guide__inset" d="M9 4h51"/>
      </g>
    </>
  ),
  runtime: (
    <>
      <g transform="matrix(1 .21 -.44 .72 36 31)">
        <path className="visual-guide__side" d="M0 0h96v55H0Z"/>
        <path className="visual-guide__face" d="M0 0h96v50H0Z"/>
      </g>
      <g transform="matrix(1 .21 -.44 .72 36 20)">
        <path className="visual-guide__side" d="M0 0h96v55H0Z"/>
        <path className="visual-guide__face" d="M0 0h96v50H0Z"/>
      </g>
      <g transform="matrix(1 .21 -.44 .72 36 9)">
        <path className="visual-guide__side" d="M0 0h96v55H0Z"/>
        <path className="visual-guide__face" d="M0 0h96v50H0Z"/>
        <path className="visual-guide__inset" d="M15 16h24m18 0h23M15 33h24m18 0h23M26 16v17m42-17v17"/>
        <path className="visual-guide__accent" d="M38 12h20v26H38Z"/>
        <path className="visual-guide__accent" d="M11 13h7v7h-7z M11 30h7v7h-7z M79 13h7v7h-7z M79 30h7v7h-7z"/>
      </g>
    </>
  ),
  artifacts: (
    <>
      <g transform="matrix(1 .21 -.44 .72 32 4)">
        <path className="visual-guide__side" d="M12 69h95l5 4H17Z M107 0l5 4v69l-5-4Z"/>
        <path className="visual-guide__face" d="M12 0h95v69H12Z"/>
        <path className="visual-guide__inset" d="M21 11h78M21 21h31v27H21Z M57 22h42m-42 8h29m-29 8h35m-35 8h22"/>
        <path className="visual-guide__accent" d="M25 31h23v13H25Z"/>
      </g>
      <g transform="matrix(1 .21 -.44 .72 85 39)">
        <path className="visual-guide__side" d="M0 46h58l4 4H4Z M58 0l4 4v46l-4-4Z"/>
        <path className="visual-guide__face" d="M0 0h58v46H0Z"/>
        <path className="visual-guide__inset" d="M0 10h58M10 19l5 4-5 4m11 0h15"/>
        <path className="visual-guide__accent" d="M10 34h30v4H10Z"/>
      </g>
    </>
  ),
  phone: (
    <>
      <g transform="matrix(1 .21 -.44 .72 68 6)">
        <path className="visual-guide__side" d="M0 0h50v96H0Z"/>
        <path className="visual-guide__face" d="M0 0h46v92H0Z"/>
        <path className="visual-guide__inset" d="M7 14h32v61H7Z M17 7h12M18 84h10 M12 24h21m-21 8h16m-16 18h21m-21 8h14"/>
        <path className="visual-guide__accent" d="M12 63h22v6H12Z"/>
      </g>
    </>
  ),
  relay: (
    <>
      <g transform="matrix(1 .21 -.44 .72 40 17)">
        <path className="visual-guide__side" d="M0 0h92v63H0Z"/>
        <path className="visual-guide__face" d="M0 0h92v57H0Z"/>
        <path className="visual-guide__inset" d="M10 16h23l11 12h10l11-12h17M10 40h23l11-12m10 0 11 12h17"/>
        <path className="visual-guide__accent" d="M39 19h20v19H39Z"/>
        <path className="visual-guide__accent" d="M6 12h8v8H6Z M6 36h8v8H6Z M78 12h8v8h-8Z M78 36h8v8h-8Z"/>
      </g>
    </>
  ),
  server: (
    <>
      <g transform="matrix(1 .21 -.44 .72 39 13)">
        <path className="visual-guide__side" d="M0 71h98l6 4H6Z M98 0l6 4v71l-6-4Z"/>
        <path className="visual-guide__face" d="M0 0h98v71H0Z"/>
        <path className="visual-guide__side" d="M8 9h82v13H8Z M8 29h82v13H8Z M8 49h82v13H8Z"/>
        <path className="visual-guide__inset" d="M26 15h47M26 35h47M26 55h47"/>
        <circle className="visual-guide__solid" cx="16" cy="15" r="2.5"/>
        <circle className="visual-guide__solid" cx="16" cy="35" r="2.5"/>
        <circle className="visual-guide__solid" cx="16" cy="55" r="2.5"/>
      </g>
    </>
  ),
  workspace: (
    <>
      <g transform="matrix(1 .21 -.44 .72 40 9)">
        <path className="visual-guide__side" d="M0 61h105l5 5H5Z M105 11l5 5v50l-5-5Z"/>
        <path className="visual-guide__face" d="M0 0h38l10 11h57v50H0Z"/>
        <path className="visual-guide__inset" d="M0 20h105 M14 31h28v18H14Z M50 32h39m-39 8h27m-27 8h39"/>
        <path className="visual-guide__accent" d="M20 36h17v8H20Z"/>
      </g>
    </>
  ),
};
