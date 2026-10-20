# Two KGEN strings of different text are different parameter values, so the
# struct instances they key are different types.
# expect: type mismatch for variable 'tag': expected Tag["None"], found Tag["Some"]
comptime KGENString = __mlir_type.`!kgen.string`


struct Tag[name: KGENString]:
    def __init__(out self):
        pass


def main():
    var tag: Tag["None".value] = Tag["Some".value]()
