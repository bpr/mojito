# `std.os` environment variables: `setenv` (with and without overwrite),
# `getenv` with a default, and `unsetenv`.
from std.os import getenv, setenv, unsetenv


def main():
    print(getenv("MOJITO_ENV_FIXTURE", "unset"))
    print(setenv("MOJITO_ENV_FIXTURE", "one"))
    print(getenv("MOJITO_ENV_FIXTURE"))
    print(setenv("MOJITO_ENV_FIXTURE", "two", overwrite=False))
    print(getenv("MOJITO_ENV_FIXTURE"))
    print(setenv("MOJITO_ENV_FIXTURE", "three"))
    print(getenv("MOJITO_ENV_FIXTURE", "unset"))
    print(unsetenv("MOJITO_ENV_FIXTURE"))
    print(getenv("MOJITO_ENV_FIXTURE", "unset"))
    print(setenv("", "x"), setenv("BAD=NAME", "x"))
    print(getenv("HOME") != "")
