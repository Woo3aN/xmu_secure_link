cmake_minimum_required(VERSION 3.16)
project(ovpnffi CXX C)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Vendored OpenVPN3 core (git submodule). Its cmake/ holds the helper macros
# (add_core_dependencies, add_ssl_library, add_json_library) we reuse.
get_filename_component(OVPN3 "${CMAKE_CURRENT_SOURCE_DIR}/../vendor/openvpn3" ABSOLUTE)
list(PREPEND CMAKE_MODULE_PATH "${OVPN3}/cmake")
include(findcoredeps)

# External-PKI helper, mirrors test/ovpncli so ovpncli.cpp's external-PKI path
# resolves at link time. We never request external PKI at runtime.
add_library(xkey STATIC
    ${OVPN3}/openvpn/openssl/xkey/xkey_helper.c
    ${OVPN3}/openvpn/openssl/xkey/xkey_provider.c)
target_include_directories(xkey PRIVATE ${OVPN3})
target_compile_definitions(xkey INTERFACE -DENABLE_EXTERNAL_PKI)
add_ssl_library(xkey)

# The data plane: OpenVPN3 ClientAPI implementation + our C-ABI shim, exported
# as a shared library so CMake resolves every transitive C++ dependency and
# Rust only has to link the single import library.
add_library(ovpnffi SHARED
    ${CMAKE_CURRENT_SOURCE_DIR}/ovpn_shim.cpp
    ${OVPN3}/client/ovpncli.cpp)
target_include_directories(ovpnffi PRIVATE ${CMAKE_CURRENT_SOURCE_DIR} ${OVPN3}/client)
add_core_dependencies(ovpnffi)
add_json_library(ovpnffi)
target_link_libraries(ovpnffi xkey)

find_package(xxHash CONFIG QUIET)
if (xxHash_FOUND)
    target_link_libraries(ovpnffi xxHash::xxhash)
endif ()

if (WIN32)
    # Enable the ovpn-dco-win data-channel-offload driver path (as in test/ovpncli).
    target_compile_definitions(ovpnffi PRIVATE ENABLE_OVPNDCOWIN)
    target_link_libraries(ovpnffi bcrypt.lib)
    set_target_properties(ovpnffi PROPERTIES WINDOWS_EXPORT_ALL_SYMBOLS ON)
endif ()

install(TARGETS ovpnffi
    RUNTIME DESTINATION bin
    LIBRARY DESTINATION lib
    ARCHIVE DESTINATION lib)
