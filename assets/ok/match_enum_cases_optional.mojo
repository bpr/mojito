# Enum case patterns over `Optional`, which declares `EnumLike`: a case
# tests `_get_enum_discriminant()` against its index in
# `_enum_case_names`, then matches its payload pattern over
# `_unsafe_get_enum_payload[case]()`. Leading-dot and qualified heads,
# refined payloads before a whole-payload case, `ref` payloads writing
# through a place, bare tag tests, nested and tuple payloads, `if case`,
# `|`, guards, a generic `Optional[T]`, and the equality a qualified head
# is over a subject that is no enum (`DType`).
@fieldwise_init
struct Holder(Copyable):
    var o: Optional[Int]


def describe(o: Optional[Int]):
    match o:
        case .Some(x):
            print("some", x)
        case .None:
            print("none")


def refine(o: Optional[Int]):
    match o:
        case Optional.Some(0):
            print("zero")
        case Optional.Some(var x):
            x += 1
            print("some+1", x)
        case Optional.None:
            print("none")


def tag[T: Copyable & Writable](o: Optional[T]):
    match o:
        case .None:
            print("generic none")
        case .Some(_):
            print("generic some")


def main():
    describe(Optional[Int](3))
    describe(None)
    refine(Optional[Int](0))
    refine(Optional[Int](4))
    refine(None)

    var o: Optional[String] = String("hi")
    match o:
        case .Some(ref s):
            s += "!"
        case .None:
            pass
    print(o.value())
    match Optional(String("tmp")):
        case .Some(ref s):
            print(s)
        case .None:
            pass
    if case .Some(v) = o:
        print("if-case", v)
    match o:
        case .Some(_) | .None:
            print("any")
    match o:
        case .Some(x) if x.byte_length() > 10:
            print("long")
        case .Some(x):
            print("short", x)
        case .None:
            print("none")

    var pair = Optional((1, 2))
    match pair:
        case .Some((a, b)):
            print("pair", a, b)
        case _:
            pass
    var nested = Optional(Optional(5))
    match nested:
        case .Some(.Some(x)):
            print("nested", x)
        case .Some(.None):
            print("inner none")
        case .None:
            print("outer none")
    var inner_none = Optional(Optional[Int]())
    match inner_none:
        case .Some(.Some(x)):
            print("nested", x)
        case .Some(.None):
            print("inner none")
        case .None:
            print("outer none")
    var text = Optional(String("ab"))
    match text:
        case .Some("ab"):
            print("literal payload")
        case _:
            pass

    var n = Optional[Int](3)
    match n:
        case .Some:
            print("bare tag")
        case _:
            print("other")
    match n:
        case Optional.Some:
            print("qualified bare tag")
        case _:
            print("other")
    match n:
        case Optional[String].Some(q):
            print("nominal owner", q)
        case _:
            pass
    if case .None = Optional[Int]():
        print("if-case none")
    var t = (Optional(1), Optional[Int]())
    match t:
        case (.Some(a), .None):
            print("tuple", a)
        case _:
            print("nope")
    var h = Holder(Optional(4))
    match h:
        case Holder(o=.Some(ref x)):
            print("field", x)
        case _:
            print("other")

    tag(Optional(1))
    tag(Optional[String]())

    var d = DType.float32
    match d:
        case DType.int64:
            print("int64")
        case DType.float32:
            print("float32")
        case _:
            print("other dtype")
