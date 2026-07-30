# Owned iteration consumes the List through the bundled `IterableOwned`
# conformance, whose associated iterator is the monomorphic `IteratorOwnedType`
# (current Mojo's owned-iterator member name).  `for var item in values^` selects
# `List.__iter__(var self)`, moving each element out of the source in turn.
@fieldwise_init
struct Thing:
    var x: Int

def main():
    var values = [Thing(1), Thing(2), Thing(3)]
    var total = 0
    for var item in values^:
        total = total + item.x
    print(total)
