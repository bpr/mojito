"""File-mode constants and predicates (`stat.h`)."""

comptime S_IFMT = 0o0170000
comptime S_IFDIR = 0o040000
comptime S_IFREG = 0o0100000
comptime S_IFLNK = 0o0120000


def S_ISLNK[intable: Intable](mode: intable) -> Bool:
    return (Int(mode) & S_IFMT) == S_IFLNK


def S_ISREG[intable: Intable](mode: intable) -> Bool:
    return (Int(mode) & S_IFMT) == S_IFREG


def S_ISDIR[intable: Intable](mode: intable) -> Bool:
    return (Int(mode) & S_IFMT) == S_IFDIR
