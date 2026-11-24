# A trait listed under its imported name and opted out under its
# module-qualified one is one trait listed and opted out: the entries are
# judged by the traits they resolve to.
# expect: trait 'Writable' must not be listed and also opted out with 'not'; remove one of the entries
from std.format import Writable
import std.format as fm


struct Handle(Writable, not fm.Writable):
    var fd: Int


def main():
    pass
