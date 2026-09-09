# expect: type mismatch for field 1 of 'MutView'
# An auto-borrowed source must satisfy the field's declared origin mutability:
# an immutable place (a read parameter) cannot feed an Origin[mut=True] slot.
@fieldwise_init
struct MutView[o: Origin[mut=True]]:
    var src: ref[o] List[Int]

def peek(data: List[Int]) -> Int:
    var v = MutView(data)
    return v.src[0]

def main():
    var source = List[Int]()
    source.append(7)
    print(peek(source))
