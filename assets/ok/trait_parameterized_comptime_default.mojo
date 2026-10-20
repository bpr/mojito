# A trait's parameterized `comptime` requirement may declare a default,
# which a conformer that omits the member takes, read with `Self` as that
# conformer, as the pin synthesizes the conformer's witness: a value
# (`get[idx: Int]: Int = idx + Self.K`) read by name and through a bound,
# including a refining trait's conformer, and a type (`E[id: Int]: AnyType`)
# read in a type annotation, upstream's `EnumLike._enum_elt_type_for_case`
# shape over the conformer's raw list of case types.
from std.builtin.variadics import TypeList, _MLIR


trait HasGet:
    comptime K: Int
    comptime get[idx: Int]: Int = idx + Self.K


trait Refined(HasGet):
    pass


@fieldwise_init
struct A(Refined):
    comptime K = 100


@fieldwise_init
struct G[n: Int](HasGet):
    comptime K = Self.n


@fieldwise_init
struct Own(HasGet):
    comptime K = 1
    comptime get[i: Int]: Int = i * 10


trait HasE:
    comptime E[id: Int]: AnyType = Int


@fieldwise_init
struct Q(HasE):
    pass


trait HasCases:
    comptime _cases: _MLIR.KGENParamListType[AnyType]
    comptime elt[id: Int]: AnyType = TypeList[Trait=AnyType, Self._cases]()[
        id
    ]


@fieldwise_init
struct O[T: AnyType](HasCases):
    comptime _cases = TypeList.of[Trait=AnyType, NoneType, Self.T].values


def show[T: HasGet]():
    print(T.get[5])


def main():
    show[A]()
    show[G[7]]()
    show[Own]()
    print(A.get[1], G[2].get[3], Own.get[4])
    comptime for i in range(2):
        print(A.get[i])
    var x: Q.E[0] = 3
    print(x)
    var s: O[String].elt[1] = "hi"
    print(s)
    var n: O[Int].elt[1] = 4
    print(n)
