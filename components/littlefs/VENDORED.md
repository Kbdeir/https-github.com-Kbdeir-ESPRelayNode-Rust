# Vendored Component

Unmodified build sources, headers, Kconfig and licenses from the official
`joltwallet/littlefs` ESP Component Registry release 1.19.2.
Upstream: https://github.com/joltwallet/esp_littlefs

Examples/test harnesses and image-tool integration are omitted. Web images use
the separately pinned littlefs-python tool and are verified against this port's
flash geometry. The local component name `littlefs` avoids auto-enabling the
esp-idf-svc 0.51 experimental wrapper, whose hardcoded i8 C-string pointers are
incompatible with this Xtensa toolchain's u8 c_char ABI. The firmware adapter
uses bindgen-generated pointer types instead. Upstream sources are not patched.
