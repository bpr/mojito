# A trait opted out under two spellings of the same trait is opted out twice.
# expect: trait 'Writable' must be opted out at most once; remove the duplicate 'not'
from std.format import Writable
import std.format as fm


struct Handle(not Writable, not fm.Writable):
    var fd: Int


def main():
    pass
