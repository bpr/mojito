# Trait inheritance / refinement `trait Bird(Animal):`.
trait Animal:
    def eat(self):
        ...

trait Bird(Animal):
    def fly(self):
        ...

@fieldwise_init
struct Sparrow(Bird):
    var age: Int
    def eat(self):
        pass
    def fly(self):
        pass
