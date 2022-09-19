# Compilation Database Generator

This tool generates a compilation database.

Build systems like CMake or Meson are able to generate a
[compilation database](https://clang.llvm.org/docs/JSONCompilationDatabase.html)
natively. Other build systems like Make have no build-in support. For the
latter this tool can generate the compilation database.

## Features

- **Incremental updates**: While building your project for the first time the
  complete database will be created. Afterwards CDBGen automatically adds new
  entries to the compilation database whenever a new file is added to the
  project and gets compiled. Similar, if arguments to the compiler change,
  then all corresponding entries are updated.
- **Multi-process safe**: In order to prevent race conditions access to the
  compilation database is synchronized between different CDBGen processes via
  `lockf(3)`.
- **Blazingly fast**: Updating a compilation database with roughly 1,000 entries
  and 1 MiB in size takes around 20 milliseconds on my i7-8650U. Thanks to
  [serde](https://serde.rs/).

## How To Install

CDBGen is written in Rust which means you first have to install Rust+Cargo e.g.
via [rustup](https://rustup.rs/) or your package manager (`dnf install cargo`
on Fedora/openSUSE or `apt install cargo` on Debian/Ubuntu). Then proceed as
follows:

```
cargo install --git https://github.com/stefan-sf/cdbgen
```

This builds and installs the CDBGen binary into `$HOME/.cargo/bin`.

## How To Use

The general idea is to use CDBGen as a wrapper, i.e., instead of executing the
compiler directly CDBGen is executed which updates the compilation database
first and afterwards executes the actual compiler. Which compiler is finally
executed is encoded into the file name of CDBGen. The executable file name is
expected to be of the pattern `cdbgen-$compiler` where `$compiler` is the
actual compiler which should be executed. For example

```
cdbgen-gcc -O2 -Wall -o foo foo.c
```

executes in the end

```
gcc -O2 -Wall -o foo foo.c
```

This requires that a symlink from `cdbgen-gcc` to the `cdbgen` binary exists.
Of course, this approach is not limited to `gcc` and works for any compiler.
The only requirement we have is that for each compiler `foobar` a symlink from
`cdbgen-foobar` to `cdbgen` exists.  For example, for the compiler
`arm-none-eabi-g++` a symlink from `cdbgen-arm-none-eabi-g++` to `cdbgen` is
required.

Typically build systems let you choose the compiler in one or another way. For
example, projects based on GNU Autotools respect the environment variables `CC`
as well as `CXX` during `configure`:

```
CC=cdbgen-gcc ./configure
```

Afterwards you proceed as usual as e.g. by invoking `make -j42` which will then
execute `cdbgen-gcc` instead of `gcc`. Subsequently `cdbgen-gcc` creates or
updates a `compile_commands.json` file and executes `gcc` finally. Thus you can
think of `cdbgen-gcc` as a wrapper around `gcc` which additionally deals with
the compilation database.

### Join Databases

A compilation database will be created/appended to in each directory where
`cdbgen` is invoked in. For a project consisting of a single build directory
this is all fine and good. However, for projects with multiple build
directories---including subdirectories---this might lead to multiple databases.
For example, building GCC 12 leads to multiple databases:

```
build-x86_64-pc-linux-gnu/fixincludes/compile_commands.json
build-x86_64-pc-linux-gnu/libcpp/compile_commands.json
build-x86_64-pc-linux-gnu/libiberty/compile_commands.json
c++tools/compile_commands.json
compile_commands.json
fixincludes/compile_commands.json
gcc/compile_commands.json
intl/compile_commands.json
libbacktrace/compile_commands.json
libcc1/compile_commands.json
libcody/compile_commands.json
libcpp/compile_commands.json
libdecnumber/compile_commands.json
libiberty/compile_commands.json
lto-plugin/compile_commands.json
```

In case a single database is preferred, set environment variable `CDBGEN` to
the _absolute_ path of the database:

```
export CDBGEN="$HOME/build/compile_commands.json"
```

## Why Yet Another Tool?

One of the most prominent tools is probably
[Bear](https://github.com/rizsotto/Bear/). The one thing I was missing for a
long time is proper support of a parallel build. Too often I was running into
[issue 443](https://github.com/rizsotto/Bear/issues/443) which motivated me to
write this small tool. If you can live with that issue, then Bear is a great
tool and you should give it a try.
