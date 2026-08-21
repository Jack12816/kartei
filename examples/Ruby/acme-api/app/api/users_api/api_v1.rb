# frozen_string_literal: true

# The users API namespace.
module UsersApi
  # The first version of the users API. Mounts every action class
  # below this namespace.
  class ApiV1 < Grape::API
    version 'v1', using: :path
    format :json

    mount UsersApi::ApiV1::Find
    mount UsersApi::ApiV1::Create
  end
end
