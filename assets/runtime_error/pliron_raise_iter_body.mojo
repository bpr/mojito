# expect: unhandled error: boom at 2
# A raise from a loop body propagates across a live iterator and its
# droppable source: the loop's cleanup destroys both before the error
# leaves `main`.
@fieldwise_init
struct Loud(Movable):
    var tag: Int

    def __deinit__(deinit self):
        print("dropped", self.tag)

def main() raises:
    var keeper = Loud(7)
    for x in range(5):
        print(x)
        if x == 2:
            raise Error("boom at 2")
    print("unreached", keeper.tag)
