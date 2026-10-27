# UI users and API users: a gentle introduction

FerroCAD has two kinds of user — **UI users** and **API users** — and they are easy to
confuse. Both work on the same document; the difference is the surface they use.

- **UI users** work in the application window: the viewport, the menus, the property
  editor and the dialogs.
- **API users** work through Python: the console inside the app, a macro file, or a
  script run with no window at all.

The distinction is not a hierarchy. Both drive the same document model. It is, however,
the distinction that explains why FerroCAD is shaped the way it is: the Python API is
the contract, and the window is one client of it.

## The same model, two front doors

| You are… | You use… | Start with |
| --- | --- | --- |
| A UI user | the app window | [the repository README](../../../README.md) and the app itself |
| An API user | the Python console, a macro, or a script | this page, then the [Python API reference](../../python-api/index.md) |
| An addon author | the API, packaged for other people | the [mod contract](../../mods/README.md) |
| A developer | the engine and the shell | [developer/](../../developer/README.md) |

A macro is not a second-class path. The console, a macro and a workbench command all
call the same `FreeCAD` and `FreeCADGui` functions. The generated pages in this guide
are **API workflows**: they run the calls a console line or a macro would, with no
window at all.

## Why the API came first

FreeCAD's history explains the shape of the project.

The project started in January 2001, when Jürgen Riegel began a graphical object
modeler on top of Cas.CADE — the commercial CAD framework that had been open-sourced in
2000 and renamed OpenCASCADE. It was registered as FreeCAD in March 2002 and announced
publicly in 2003. Werner Mayer joined early, and the two split the work: Jürgen on the
application and document logic, Werner on the GUI.

The first version used OpenCASCADE's own document framework, OCAF. After a year or two
the team hit a dead end: the documentation was poor and they could not extend OCAF with
their own feature types. They kept the modelling kernel and wrote their own
application/document framework. That decision is still visible: OpenCASCADE supplies
geometry; FreeCAD owns the document, the feature types and the Python surface.

The 2003 announcement was explicit about the audience. FreeCAD was described as an
open-source CAx RAD platform, with macro recording, workbenches and the ability to run
as a server or a loadable extension. Its first audience, Jürgen wrote, would be CAx
developers who needed groundwork for their own development. It was a technical demo and
a platform for building CAD applications, not a finished end-user tool with a polished
interface.

Python was part of that platform idea from early on. Werner later named the key design
decision: expose all important functions to Python so power users could extend FreeCAD
with their own functions. Yorik van Havre's Draft workbench, written entirely in Python
and shipped in version 0.7 in 2009, proved that the extension path was real: a
workbench could be built on the API rather than in the C++ core.

## The commercial attempt

In 2025, Ondsel — a company that tried to build a commercial CAD application around
FreeCAD — announced it was shutting down after almost two years. It had contributed
real work upstream: an assembly workbench and its solver, usability work in Sketcher
and TechDraw, VarSets, and a more polished interface. But it could not find
product-market fit or enough commercial adoption, and it ran out of runway.

The lesson for FerroCAD is not that the UI does not matter. It is that the **durable
surface is the open platform and its API**. A commercial front end can come and go; the
document model, the Python API and the workbenches are what people build on. The UI is
a client of that surface, not the other way around.

## What that means in practice

- **If you want to do one thing once**, use the window.
- **If you want to repeat it, automate it, or build on it**, use the Python console or a
  macro. The API is the same one the window uses.
- **If you want other people to use it**, package it as a mod or workbench: that is the
  addon contract in [`mods/`](../../mods/README.md).
- **If you want to change how FerroCAD works**, the engine and shell notes are in
  [`developer/`](../../developer/README.md).

The UI and the API are not competing products. They are two front doors to the same
document, and the API is the one designed to stay stable.

## Sources

- Jürgen Riegel, “Who is behind FreeCAD?” (FreeCAD wiki).
- Jürgen Riegel, the FreeCAD announcement on the OpenCASCADE forum, 17 August 2003.
- Werner Mayer's account in the FreeCAD forum:
  <https://forum.freecad.org/viewtopic.php?f=13&t=40235&start=10#p342330>.
- Ondsel, “We are shutting down Ondsel”, 30 October 2025.
