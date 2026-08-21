-module(sample).
-export([area/1, area/2]).

-record(point, {x, y}).

-define(PI, 3.14159).
-define(SQUARE(X), (X * X)).

-type shape() :: circle | square.

area(#point{x = X}) ->
    X.

area(circle, Radius) ->
    ?PI * Radius * Radius;
area(square, Side) ->
    ?SQUARE(Side).
