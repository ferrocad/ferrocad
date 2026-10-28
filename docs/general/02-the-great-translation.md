# Chapter 2: The great translation (workbenches, viewports and GPUs)

Here is the fundamental limitation of modern computer hardware: **graphics cards cannot
read or draw B-Rep or NURBS equations.**

Low-level graphics APIs such as **OpenGL** and **Vulkan** are built to do one thing at
blistering speed: process millions of flat triangles in parallel. To display a
mathematical CAD model, the software must translate it on the fly. That translation is
called **tessellation**.

### 2.1 The data pipeline

![The CAD data pipeline: NURBS and B-Rep are tessellated into a temporary mesh; a permanent mesh arrives from STL or OBJ; both feed the viewport scene graph and the GPU.](images/tessellation-pipeline.svg)

*The translation from mathematical geometry to triangles, and the two routes into the viewport.*

### 2.2 How FreeCAD workbenches differ

- **Part / Part Design (vector workflow).** Stores true B-Rep math through OCCT. Every
  time you modify a dimension, FreeCAD silently tessellates the B-Rep into a
  **temporary visual mesh** and hands it to **Coin3D**, the Open Inventor viewport
  engine, which Python reaches through **Pivy**.
- **Mesh (bitmap workflow).** Holds a **permanent mesh**. Because the data is already
  made of triangles, FreeCAD skips the mathematical kernel and passes the triangles
  straight to Coin3D.

### 2.3 Down to the silicon: shaders

Once Coin3D organizes the triangles into scene-graph nodes such as
`SoIndexedFaceSet`, it packs the coordinates into memory arrays (**vertex buffer
objects**) and sends them to OpenGL/Vulkan:

1. **The vertex shader** runs on every triangle corner and calculates where it sits in
   2D screen space for the current camera.
2. **The fragment shader** colours the pixels between those corners. To keep the model
   from looking like a blocky disco ball, the viewport calculates **vertex normals**
   perpendicular to the original NURBS surface. The fragment shader blends the lighting
   across the flat triangles, creating the optical illusion of a smooth, glossy
   surface.
