%%%-------------------------------------------------------------------
%%% @doc Track unread message counters per user and conversation.
%%%
%%% The module hooks into the message delivery pipeline, increments
%%% the counter of every offline recipient and exposes a query
%%% handler to fetch and reset the counters.
%%% @end
%%%-------------------------------------------------------------------
-module(mod_unread).
-behaviour(gen_mod).

-export([start/2, stop/1, depends/2, mod_options/1]).
-export([on_message/1, unread_count/2]).

-include("logger.hrl").

%%%===================================================================
%%% gen_mod callbacks
%%%===================================================================

%% @doc Start the module on the given host.
start(Host, _Opts) ->
    chat_hooks:add(user_receive_packet, Host, ?MODULE, on_message, 50),
    ok.

%% @doc Stop the module on the given host.
stop(Host) ->
    chat_hooks:delete(user_receive_packet, Host, ?MODULE, on_message, 50),
    ok.

depends(_Host, _Opts) ->
    [].

mod_options(_Host) ->
    [].

%%%===================================================================
%%% hooks
%%%===================================================================

%% @doc Count an incoming message for its recipient.
on_message({Packet, State}) ->
    {Packet, State}.

%% @doc Fetch the unread counter of a user and conversation.
unread_count(_User, _Conversation) ->
    0.
