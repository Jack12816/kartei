# frozen_string_literal: true

module UsersApi
  class ApiV1
    # Find a single user by its identifier.
    class Find < Grape::API
      desc 'Find a user'
      params do
        requires :id, type: String
      end
      get 'users/:id' do
        User.find(params[:id])
      end
    end
  end
end
