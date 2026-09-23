# A per-instantiation method clone inherits its checked template's facts when
# the body calls its own `def(...)` parameter, or forwards it to a sibling
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `CALLABLE_PARAMETERS`). A call through a callable parameter records a
# call-through residue on the body's own frame, which names the parameter's
# slot and each argument's signature place and nothing about a type, so the
# derived instance republishes the template's residue verbatim. A caller that
# forwards its parameter reads that residue and composes one of its own; the
# instance owes that its realized callee publishes the residue the template
# read. The call's own contract symbol and parameters are taken from the
# instance's binding of the parameter.
struct Cell[T: Movable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def visit(self, handler: def(element: Self.T) thin, /):
        handler(self.item)

    def visit_at(self, index: Int, handler: def(position: Int, element: Self.T) thin, /):
        handler(index, self.item)


struct Pair[T: Movable & Deinitable](Movable):
    var first: Cell[Self.T]
    var second: Cell[Self.T]

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.first = Cell[Self.T](first^)
        self.second = Cell[Self.T](second^)

    def visit(self, handler: def(element: Self.T) thin, /):
        self.first.visit(handler)
        self.second.visit(handler)

    def visit_at(self, handler: def(position: Int, element: Self.T) thin, /):
        self.first.visit_at(0, handler)
        self.second.visit_at(1, handler)


def show_int(element: Int):
    print(element)


def show_string(element: String):
    print(element)


def show_int_at(position: Int, element: Int):
    print(position, element)


def show_string_at(position: Int, element: String):
    print(position, element)


def main():
    var p = Pair[Int](3, 4)
    p.visit(show_int)
    p.visit_at(show_int_at)
    var q = Pair[String](String("a"), String("b"))
    q.visit(show_string)
    q.visit_at(show_string_at)
