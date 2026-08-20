# A body-local droppable value is destroyed exactly once on the raising
# edge, before the handler observes anything.
struct Tracked:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("drop", self.id)

def blow() raises:
    raise Error("edge")

def main():
    try:
        var t: Tracked = Tracked(10)
        print("raising")
        blow()
        print("unreached", t.id)
    except:
        print("caught")
