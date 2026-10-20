# A string literal's `.value` is its `!kgen.string` parameter, a
# compile-time value a parameter of that MLIR type binds. Spelled directly,
# the MLIR type conforms to no trait, while a trait-bounded binder binds it
# and answers by its shape.
comptime KGENString = __mlir_type.`!kgen.string`
comptime top = "Top".value


def bind[s: KGENString](label: String):
    print(label)


struct Names:
    comptime KS = __mlir_type.`!kgen.string`
    comptime none = "None".value
    comptime some: KGENString = "Some".value


struct Tag[name: KGENString]:
    def __init__(out self):
        pass

    def show(self, label: String):
        bind[Self.name](label)


def bind_spelled[s: __mlir_type.`!kgen.string`](label: String):
    print(label)


def bind_member[s: Names.KS](label: String):
    print(label)


def forward[s: KGENString = "x".value](label: String):
    bind[s](label)


def generic_local[n: Int]():
    comptime w = "Local".value
    bind[w]("generic local")


def raw[names: __mlir_type[`!kgen.param_list<`, KGENString, `>`]]() -> Int:
    return ParameterList[names]().size


def count[*names: KGENString]() -> Int:
    bind[names[0]]("element")
    return raw[names.values]()


def conformance[T: AnyType]():
    comptime if conforms_to(T, Movable):
        print("binder movable")
    else:
        print("binder not movable")


def main():
    comptime v = "None".value
    bind[v]("alias")
    bind["None".value]("literal")
    bind[("So" + "me").value]("folded")
    bind[top]("module")
    bind_spelled[Names.none]("member")
    bind_member[Names.some]("member alias")
    forward("default")
    forward["y".value]("forwarded")
    generic_local[1]()
    var tag: Tag["Some".value] = Tag[Names.some]()
    tag.show("tag")
    print(count["None".value, "Some".value]())
    comptime if conforms_to(KGENString, AnyType):
        print("any")
    else:
        print("not any")
    conformance[KGENString]()
