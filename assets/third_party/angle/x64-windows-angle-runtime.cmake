set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE dynamic)
set(VCPKG_BUILD_TYPE release)

string(SUBSTRING "${VCPKG_ROOT_DIR}" 0 2 ANGLE_BUILD_DRIVE)
set(ANGLE_PATHMAP_FLAGS "/experimental:deterministic /pathmap:${ANGLE_BUILD_DRIVE}=angle-build")
set(VCPKG_C_FLAGS "${ANGLE_PATHMAP_FLAGS}")
set(VCPKG_CXX_FLAGS "${ANGLE_PATHMAP_FLAGS}")
set(VCPKG_LINKER_FLAGS "/PDBALTPATH:%_PDB%")

# ANGLE itself must remain a pair of DLLs, while its zlib dependency is linked
# into libGLESv2 so the deployed runtime stays limited to libEGL/libGLESv2.
if(PORT STREQUAL "angle")
    set(VCPKG_LIBRARY_LINKAGE dynamic)
else()
    set(VCPKG_LIBRARY_LINKAGE static)
endif()

set(VCPKG_PROVIDED_FORTRAN ON)
