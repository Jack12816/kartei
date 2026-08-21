# frozen_string_literal: true

module UsersApi
  class ApiV1
    # Create a new user from the given attributes.
    class Create < Grape::API
      desc 'Create a user'
      params do
        requires :email, type: String
      end
      post 'users' do
        User.create!(declared(params))
      end
    end
  end
end
