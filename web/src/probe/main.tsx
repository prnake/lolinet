import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "../styles/tokens.css";
import "../styles/base.css";
import "../styles/motion.css";
import "../styles/components.css";
import "../styles/app.css";
import "../styles/probe.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ProbeApp } from "./ProbeApp";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ProbeApp />
  </StrictMode>,
);
