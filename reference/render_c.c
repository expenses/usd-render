#include <GL/freeglut.h>
#include <openusd-c.h>
#include <stdio.h>

int main(int argc, char** argv) {
  glutInit(&argc, argv);
  glutInitDisplayMode(GLUT_RGBA);
  glutInitWindowSize(500, 500);
  glutInitWindowPosition(0, 0);
  int window = glutCreateWindow("GLLLL");
  
  if (argc != 2) {
    return 1;
  }

  char* filename = argv[1];

  usd_StageRefPtr_t* stage;
  int result = usd_Stage_Open(filename, 0, &stage);

  printf("%i\n", result);

  usd_Prim_t* prim;
  usd_StageRefPtr_GetPseudoRoot(stage, &prim);

  sdf_Path_t* path;
  sdf_Path_from_string("/camera1", &path);

  usd_Prim_t* camera_prim;
  usd_StageRefPtr_GetPrimAtPath(stage, path, &camera_prim);

  usdGeom_Camera_t* camera;
  usdGeom_Camera_new(camera_prim, &camera);

  gf_Camera_t* gf_camera;
  usd_TimeCode_t time;
  usd_TimeCode_Default(&time);
  usdGeom_Camera_GetCamera(camera, &time, &gf_camera);

  gf_Frustum_t* frustum;
  gf_Camera_GetFrustum(gf_camera, &frustum);

  gf_Matrix4d_t proj;
  gf_Matrix4d_t view;
  gf_Frustum_ComputeProjectionMatrix(frustum, &proj);
  gf_Frustum_ComputeViewMatrix(frustum, &view);

  usdImaging_GLEngine_t* engine;
  usdImaging_GLEngine_new(&engine);

  usdImaging_GLEngine_SetCameraState(engine, &view, &proj);  

  void showScreen() {
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
    gf_Vec4d_t viewport;
    viewport.x = 0.0;
    viewport.y = 0.0;
    viewport.z = glutGet(GLUT_WINDOW_WIDTH);
    viewport.w = glutGet(GLUT_WINDOW_HEIGHT);
    usdImaging_GLEngine_SetRenderViewport(engine, &viewport);

    usdImaging_render(engine, prim);
    glutSwapBuffers();
  }

  glClearColor(0.1, 0.2, 0.3, 1.0);

  glutDisplayFunc(showScreen);
  glutIdleFunc(showScreen);
  glutMainLoop();

  return 0;
}
