# expect: invalid redefinition of 'List'
# A module cannot declare a name it imports explicitly: the import already
# binds it, as the pin's "cannot define a struct here with name 'List'"
# says. Without the import, the declaration would shadow the prelude's.
from std.collections.list import List


struct List:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def main():
    print(List(3).x)
