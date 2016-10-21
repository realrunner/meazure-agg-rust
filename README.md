# meazure-agg-rust

Compiling rust-openssl on windows is a pain and you need to do more than just follow its readme.
Currenty hyper v0.9.6 uses a version of rust-openssl that does _not_ support OpenSSL 1.1, so you have to install OpenSSL 1.0.

See instructions here:
https://gist.github.com/retep998/eeca39710290d294c81f38e8f8490801

* download OpenSSL 1.0 64bit variant to C:\OpenSSL-Win64
* set OPENSSL_LIB_DIR = C:\OpenSSL-Win64\lib
* set OPENSSL_INCLUDE_DIR = C:\OpenSSL-Win64\include
* set OPENSSL_LIBS = libeay32:ssleay32
* set PATH=%PATH%;C:\OpenSSL-Win64\bin

