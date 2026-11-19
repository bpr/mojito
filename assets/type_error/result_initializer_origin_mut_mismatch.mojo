# `Origin`'s result-form initializer constructs an immutable origin, so an
# explicit `mut=True` disagrees with its declared result and no initializer
# matches.
# expect: no constructor overload matches the supplied arguments


def main():
    var d = Origin[mut=True](MutUntrackedOrigin)
    _ = d
