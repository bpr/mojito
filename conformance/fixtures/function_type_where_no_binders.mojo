# mojo-only (strict-subset gap): the audited head accepts a trailing `where`
# clause on a function type that declares NO `def[...]` parameters of its own
# (the clause may be trivially true or reference enclosing binders). Mojito
# requires the contract to declare binders — its clauses lower onto the
# anonymous contract's parameter declarations, and a monomorphic `Ty::Func`
# has nowhere to carry them ("a function-type 'where' clause requires a
# def[...] parameter list").
def use(x: Int):
    print(x)

def apply[F: def(Int) thin -> None where (True, "always")](x: Int):
    F(x)

def main():
    apply[use](3)
