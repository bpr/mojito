"""Operating-system services: directories, files, environment variables,
and path predicates (subset of upstream's `std.os`)."""

from .env import getenv, setenv, unsetenv
from .os import (
    SEEK_CUR,
    SEEK_END,
    SEEK_SET,
    abort,
    listdir,
    makedirs,
    mkdir,
    remove,
    removedirs,
    rmdir,
    sep,
    unlink,
)
from .pathlike import PathLike

from . import path
