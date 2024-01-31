from OpenGL.GL import *
from OpenGL.GLUT import *
from OpenGL.GLU import *
from pxr import UsdImagingGL, Usd, Gf, UsdGeom
import sys

glutInit()
glutInitDisplayMode(GLUT_RGBA)
glutInitWindowSize(500, 500)
glutInitWindowPosition(0, 0)
wind = glutCreateWindow("OpenGL Coding Practice")

filename = sys.argv[1]

stage = Usd.Stage.Open(filename)

prim = stage.GetPseudoRoot()

camera_prim = stage.GetPrimAtPath("/camera1")

camera = UsdGeom.Camera(camera_prim)
gf_camera = camera.GetCamera()
frustum = gf_camera.frustum
proj = frustum.ComputeProjectionMatrix()
view = frustum.ComputeViewMatrix()

engine = UsdImagingGL.Engine()
engine.SetRendererAov("color")

params = UsdImagingGL.RenderParams()
params.enableLighting = False

engine.SetCameraState(view, proj)

glClearColor(0.1, 0.2, 0.3, 1.0)

def showScreen():
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT) # Remove everything from screen (i.e. displays all white)
    engine.SetRenderViewport(Gf.Vec4d(0, 0, glutGet(GLUT_WINDOW_WIDTH), glutGet(GLUT_WINDOW_HEIGHT)))
    engine.Render(prim, params)
    glutSwapBuffers()

glutDisplayFunc(showScreen)  # Tell OpenGL to call the showScreen method continuously
glutIdleFunc(showScreen)     # Draw any graphics or shapes in the showScreen function at all times
glutMainLoop()  # Keeps the window created above displaying/running in a loop
